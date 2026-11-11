use ratatui::widgets::ListState;
use std::collections::VecDeque;

pub const MANAGER_NAMES: &[&str] = &[
    "npm", "pnpm", "yarn", "bun", "pip", "pipx", "poetry", "uv", "cargo", "rustup", "flutter",
    "brew", "winget",
];

#[derive(Clone, Debug)]
pub struct PackageManager {
    pub name: String,
    pub path: Option<String>,
    pub version: Option<String>,
    pub detected: bool,
    pub selected: bool,
}

impl PackageManager {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            path: None,
            version: None,
            detected: false,
            selected: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DetectedInfo {
    pub path: String,
    pub version: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Debug)]
pub enum AppEvent {
    DetectionCompleted {
        index: usize,
        result: Result<Option<DetectedInfo>, String>,
    },
    ScanFinished,
    Log(String),
    Progress(Option<String>),
    UpdateStarted {
        index: usize,
        position: usize,
        total: usize,
    },
    UpdateFinished(Vec<String>),
}

pub struct App {
    pub items: Vec<PackageManager>,
    pub list_state: ListState,
    pub select_all_state: bool,
    pub scanning: bool,
    pub updating: bool,
    pub status: String,
    pub logs: VecDeque<String>,
    pub progress: Option<String>,
    pub updating_index: Option<usize>,
    pub update_position: usize,
    pub update_total: usize,
}

impl App {
    pub fn new() -> Self {
        Self {
            items: MANAGER_NAMES
                .iter()
                .map(|name| PackageManager::new(name))
                .collect(),
            list_state: ListState::default().with_selected(Some(0)),
            select_all_state: false,
            scanning: true,
            updating: false,
            status: "Scanning for package managers...".to_owned(),
            logs: VecDeque::new(),
            progress: None,
            updating_index: None,
            update_position: 0,
            update_total: 0,
        }
    }

    pub fn selected_index(&self) -> usize {
        self.list_state.selected().unwrap_or(0)
    }

    pub fn move_down(&mut self) {
        self.list_state
            .select(Some((self.selected_index() + 1).min(self.items.len())));
    }

    pub fn move_up(&mut self) {
        self.list_state
            .select(Some(self.selected_index().saturating_sub(1)));
    }

    pub fn toggle_current(&mut self) {
        let index = self.selected_index();
        if index == 0 {
            self.toggle_select_all();
        } else if let Some(item) = self.items.get_mut(index - 1) {
            if item.detected {
                item.selected = !item.selected;
            }
            if self.scanning {
                self.select_all_state = false;
            } else {
                self.refresh_select_all_state();
            }
        }
    }

    pub fn toggle_select_all(&mut self) {
        let has_detected = self.items.iter().any(|item| item.detected);
        if !has_detected && !self.scanning {
            return;
        }
        self.select_all_state = !self.select_all_state;
        for item in &mut self.items {
            item.selected = item.detected && self.select_all_state;
        }
    }

    pub fn apply_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::DetectionCompleted { index, result } => {
                let mut log_message = None;
                if let Some(item) = self.items.get_mut(index) {
                    match result {
                        Ok(Some(info)) => {
                            item.path = Some(info.path);
                            item.version = info.version;
                            item.detected = true;
                            item.selected = self.select_all_state;
                            log_message = info
                                .diagnostic
                                .map(|diagnostic| format!("{}: {diagnostic}", item.name));
                        }
                        Ok(None) => {}
                        Err(error) => {
                            log_message = Some(format!("{}: detection failed: {error}", item.name));
                        }
                    }
                }
                if let Some(message) = log_message {
                    self.push_log(message);
                }
            }
            AppEvent::ScanFinished => {
                self.scanning = false;
                self.refresh_select_all_state();
                let count = self.items.iter().filter(|item| item.detected).count();
                self.status = format!("Scan complete: {count} package manager(s) detected.");
            }
            AppEvent::Log(message) => self.push_log(message),
            AppEvent::Progress(progress) => self.progress = progress,
            AppEvent::UpdateStarted {
                index,
                position,
                total,
            } => {
                self.updating_index = Some(index);
                self.update_position = position;
                self.update_total = total;
                self.list_state.select(Some(index + 1));
                self.status = format!(
                    "Updating {} ({position}/{total})...",
                    self.items
                        .get(index)
                        .map(|item| item.name.as_str())
                        .unwrap_or("package manager")
                );
            }
            AppEvent::UpdateFinished(failures) => {
                self.updating = false;
                self.progress = None;
                self.updating_index = None;
                self.update_position = 0;
                self.update_total = 0;
                self.status = if failures.is_empty() {
                    "All selected package manager updates completed successfully.".to_owned()
                } else {
                    let first_failure = failures.first().map(String::as_str).unwrap_or_default();
                    format!(
                        "{} package manager update(s) failed. First: {first_failure}",
                        failures.len()
                    )
                };
            }
        }
    }

    pub fn selected_items(&self) -> Vec<PackageManager> {
        self.items
            .iter()
            .filter(|item| item.detected && item.selected)
            .cloned()
            .collect()
    }

    pub fn push_log(&mut self, message: String) {
        const MAX_LOG_LINES: usize = 200;
        self.logs.push_back(message);
        if self.logs.len() > MAX_LOG_LINES {
            self.logs.pop_front();
        }
    }

    fn refresh_select_all_state(&mut self) {
        let detected: Vec<_> = self.items.iter().filter(|item| item.detected).collect();
        self.select_all_state = !detected.is_empty() && detected.iter().all(|item| item.selected);
    }
}

#[cfg(test)]
mod tests {
    use super::{App, AppEvent, DetectedInfo};

    #[test]
    fn select_all_affects_only_detected_managers() {
        let mut app = App::new();
        app.apply_event(AppEvent::DetectionCompleted {
            index: 0,
            result: Ok(Some(DetectedInfo {
                path: "/usr/bin/npm".to_owned(),
                version: Some("10.0".to_owned()),
                diagnostic: None,
            })),
        });

        app.toggle_select_all();

        assert!(app.items[0].selected);
        assert!(!app.items[1].selected);
        assert!(app.select_all_state);
    }

    #[test]
    fn navigation_includes_select_all_row_and_clamps() {
        let mut app = App::new();

        app.move_up();
        assert_eq!(app.selected_index(), 0);

        for _ in 0..(app.items.len() + 2) {
            app.move_down();
        }
        assert_eq!(app.selected_index(), app.items.len());
    }

    #[test]
    fn select_all_during_scan_includes_later_detections() {
        let mut app = App::new();
        app.toggle_select_all();
        app.apply_event(AppEvent::DetectionCompleted {
            index: 0,
            result: Ok(Some(DetectedInfo {
                path: "/usr/bin/npm".to_owned(),
                version: None,
                diagnostic: None,
            })),
        });

        assert!(app.select_all_state);
        assert!(app.items[0].selected);

        app.apply_event(AppEvent::ScanFinished);
        assert!(app.select_all_state);
    }

    #[test]
    fn progress_updates_replace_the_current_progress_state() {
        let mut app = App::new();
        app.updating = true;
        app.apply_event(AppEvent::Progress(Some(
            "bun: Downloading [51.61MiB]".to_owned(),
        )));
        app.apply_event(AppEvent::Progress(Some(
            "bun: Downloading [52.70MiB]".to_owned(),
        )));

        assert_eq!(app.progress.as_deref(), Some("bun: Downloading [52.70MiB]"));
        assert!(app.logs.is_empty());

        app.apply_event(AppEvent::UpdateFinished(Vec::new()));
        assert!(app.progress.is_none());
    }

    #[test]
    fn starting_an_update_focuses_manager_without_clearing_selection() {
        let mut app = App::new();
        app.items[1].detected = true;
        app.items[1].selected = true;

        app.apply_event(AppEvent::UpdateStarted {
            index: 1,
            position: 2,
            total: 3,
        });

        assert_eq!(app.selected_index(), 2);
        assert!(app.items[1].selected);
        assert_eq!(app.updating_index, Some(1));
        assert_eq!(app.status, "Updating pnpm (2/3)...");
    }
}
