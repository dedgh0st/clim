mod app;
mod detector;
mod executor;
mod ui;

use app::{App, AppEvent};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, IsTerminal, Stdout, Write};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide) {
            let _ = disable_raw_mode();
            return Err(error);
        }

        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
                let _ = disable_raw_mode();
                return Err(error);
            }
        };
        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = self.terminal.show_cursor();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return run_headless().await;
    }

    let mut session = TerminalSession::new()?;
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let mut app = App::new();
    let scan_sender = sender.clone();
    let scan_task = tokio::spawn(async move {
        detector::scan(scan_sender).await;
    });

    let mut update_task: Option<JoinHandle<()>> = None;
    let mut events = EventStream::new();
    let mut redraw = tokio::time::interval(Duration::from_millis(100));

    loop {
        ui::draw(&mut session.terminal, &mut app)?;
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        if handle_key(key.code, &mut app, &sender, &mut update_task) {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error.into()),
                    None => break,
                }
            }
            Some(event) = receiver.recv() => {
                let finished_update = matches!(event, AppEvent::UpdateFinished(_));
                app.apply_event(event);
                if finished_update {
                    update_task = None;
                }
            }
            _ = redraw.tick() => {}
        }
    }

    scan_task.abort();
    if let Some(task) = update_task {
        task.abort();
    }
    Ok(())
}

async fn run_headless() -> Result<(), Box<dyn std::error::Error>> {
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let mut app = App::new();
    detector::scan(sender).await;

    while let Some(event) = receiver.recv().await {
        app.apply_event(event);
    }

    let mut report = String::from("CLIM - Universal Package Manager Manager\n");
    report.push_str(&format!("{}\n\n", app.status));
    let detected: Vec<_> = app.items.iter().filter(|item| item.detected).collect();
    if detected.is_empty() {
        report.push_str("No supported package managers were detected on PATH.\n");
    } else {
        report.push_str("Detected package managers:\n");
        for item in detected {
            let version = item.version.as_deref().unwrap_or("version unavailable");
            let path = item.path.as_deref().unwrap_or("path unavailable");
            report.push_str(&format!("  {:<8} {:<24} {}\n", item.name, version, path));
        }
    }

    if !app.logs.is_empty() {
        report.push_str("\nDiagnostics:\n");
        for message in &app.logs {
            report.push_str(&format!("  {message}\n"));
        }
    }
    report.push_str("\nInteractive updates require a terminal. Run `clim` in a terminal to select and update managers.\n");
    io::stdout().write_all(report.as_bytes())?;
    Ok(())
}

fn handle_key(
    key: KeyCode,
    app: &mut App,
    sender: &mpsc::UnboundedSender<AppEvent>,
    update_task: &mut Option<JoinHandle<()>>,
) -> bool {
    match key {
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => return true,
        _ if app.updating => {}
        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
        KeyCode::Char(' ') => app.toggle_current(),
        KeyCode::Char('a') | KeyCode::Char('A') => app.toggle_select_all(),
        KeyCode::Enter | KeyCode::Char('u') | KeyCode::Char('U') => {
            if app.scanning {
                app.status = "Wait for detection to finish before updating.".to_owned();
            } else {
                let selected = app.selected_items();
                if selected.is_empty() {
                    app.status = "Select at least one detected package manager.".to_owned();
                } else {
                    app.updating = true;
                    app.status = "Updating selected package managers...".to_owned();
                    let sender = sender.clone();
                    *update_task = Some(tokio::spawn(async move {
                        executor::update_selected(selected, sender).await;
                    }));
                }
            }
        }
        _ => {}
    }
    false
}
