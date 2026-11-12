use crate::app::App;
use crate::app::UpdateAvailability;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

pub fn draw<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> std::io::Result<()> {
    terminal.draw(|frame| render(frame, app))?;
    Ok(())
}

fn render(frame: &mut Frame<'_>, app: &mut App) {
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let header = Paragraph::new("CLIM - Universal Package Manager Manager")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, regions[0]);

    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(regions[1]);

    let mut rows = Vec::with_capacity(app.items.len() + 1);
    let select_label = if app.select_all_state {
        "Deselect All"
    } else {
        "Select All"
    };
    rows.push(ListItem::new(Line::from(vec![
        Span::styled(
            checkbox(app.select_all_state),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(format!(" {select_label}")),
    ])));

    rows.extend(app.items.iter().enumerate().map(|(index, item)| {
        let checkbox_color = if item.detected {
            Color::Green
        } else {
            Color::DarkGray
        };
        let mut details = if item.detected {
            item.path
                .clone()
                .unwrap_or_else(|| "path unavailable".to_owned())
        } else {
            "not detected".to_owned()
        };
        if let Some(version) = &item.version {
            details.push_str("  ");
            details.push_str(version);
        }

        let availability = item.update_availability.as_ref().map(|state| match state {
            UpdateAvailability::Checking => ("Checking...".to_owned(), Color::Cyan),
            UpdateAvailability::UpToDate { .. } => ("Up to date".to_owned(), Color::Green),
            UpdateAvailability::Available { latest, .. } => {
                (format!("Update: {latest}"), Color::Yellow)
            }
            UpdateAvailability::Unavailable(_) => ("Check unavailable".to_owned(), Color::DarkGray),
            UpdateAvailability::Failed(_) => ("Check failed".to_owned(), Color::Red),
        });
        let mut line = vec![
            Span::styled(checkbox(item.selected), Style::default().fg(checkbox_color)),
            Span::styled(
                format!(" {:<10}", item.name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some((label, color)) = availability {
            line.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
        }
        line.push(Span::styled(
            format!(" {details}"),
            Style::default().fg(Color::DarkGray),
        ));
        if app.updating_index == Some(index) {
            line.push(Span::styled(
                "  << UPDATING",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        ListItem::new(Line::from(line))
    }));

    let selected_count = app.items.iter().filter(|item| item.selected).count();
    let list = List::new(rows)
        .block(
            Block::default()
                .title(format!("Package managers ({selected_count} selected)"))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("» ");
    frame.render_stateful_widget(list, body[0], &mut app.list_state);

    let mut log_lines = vec![Line::from(Span::styled(
        app.status.as_str(),
        Style::default().fg(Color::Yellow),
    ))];
    if app.updating {
        if let Some(progress) = &app.progress {
            log_lines.push(Line::from(Span::styled(
                progress.as_str(),
                Style::default().fg(Color::Cyan),
            )));
        }
    }
    let visible_log_lines = body[1].height.saturating_sub(3) as usize;
    log_lines.extend(
        app.logs
            .iter()
            .rev()
            .take(visible_log_lines)
            .rev()
            .map(|line| Line::from(line.as_str())),
    );
    let log = Paragraph::new(log_lines)
        .block(
            Block::default()
                .title("Status / execution log")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(log, body[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::styled("[↑/↓] ", Style::default().fg(Color::Yellow)),
        Span::raw("Navigate  "),
        Span::styled("[Space] ", Style::default().fg(Color::Yellow)),
        Span::raw("Toggle  "),
        Span::styled("[A] ", Style::default().fg(Color::Yellow)),
        Span::raw("Select All  "),
        Span::styled("[C] ", Style::default().fg(Color::Yellow)),
        Span::raw("Check Updates  "),
        Span::styled("[Enter] ", Style::default().fg(Color::Yellow)),
        Span::raw(if app.updating {
            format!("Updating {}/{}  ", app.update_position, app.update_total)
        } else {
            "Update Selected  ".to_owned()
        }),
        Span::styled("[Q] ", Style::default().fg(Color::Yellow)),
        Span::raw("Quit"),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, regions[2]);
}

fn checkbox(checked: bool) -> &'static str {
    if checked {
        "[x]"
    } else {
        "[ ]"
    }
}
