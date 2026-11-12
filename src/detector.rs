use crate::app::{AppEvent, DetectedInfo, MANAGER_NAMES};
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinSet;
use tokio::time::timeout;

pub async fn scan(sender: UnboundedSender<AppEvent>) {
    let mut tasks = JoinSet::new();
    for (index, name) in MANAGER_NAMES.iter().enumerate() {
        let sender = sender.clone();
        let name = (*name).to_owned();
        tasks.spawn(async move {
            let result = detect(&name).await;
            let _ = sender.send(AppEvent::DetectionCompleted { index, result });
        });
    }

    while tasks.join_next().await.is_some() {}
    let _ = sender.send(AppEvent::ScanFinished);
}

async fn detect(name: &str) -> Result<Option<DetectedInfo>, String> {
    let Some(path) = find_binary(name) else {
        return Ok(None);
    };

    let mut version = None;
    let mut diagnostic = None;
    for flag in ["--version", "-v"] {
        let mut command = command_for(&path, &[flag]);
        command.kill_on_drop(true);
        let result = match timeout(Duration::from_secs(10), command.output()).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                diagnostic = Some(format!("version check failed: {error}"));
                continue;
            }
            Err(_) => {
                diagnostic = Some("version check timed out".to_owned());
                continue;
            }
        };

        if result.status.success() {
            let output = if result.stdout.is_empty() {
                &result.stderr
            } else {
                &result.stdout
            };
            let text = String::from_utf8_lossy(output).trim().to_owned();
            if !text.is_empty() {
                version = Some(text.lines().next().unwrap_or_default().to_owned());
                break;
            }
        } else {
            diagnostic = Some(format!("version check exited with {}", result.status));
        }
    }
    if version.is_none() && diagnostic.is_none() {
        diagnostic = Some("executable found, but its version could not be read".to_owned());
    }

    Ok(Some(DetectedInfo {
        path: path.to_string_lossy().into_owned(),
        version,
        diagnostic,
    }))
}

pub fn find_binary(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for directory in env::split_paths(&path_var) {
        for candidate in binary_candidates(&directory, name) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn binary_candidates(directory: &Path, name: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let extensions = env::var_os("PATHEXT")
            .map(|value| {
                value
                    .to_string_lossy()
                    .split(';')
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![".EXE".to_owned(), ".CMD".to_owned(), ".BAT".to_owned()]);

        let already_has_extension = Path::new(name).extension().is_some();
        if already_has_extension {
            vec![directory.join(name)]
        } else {
            std::iter::once(directory.join(name))
                .chain(
                    extensions
                        .iter()
                        .map(|extension| directory.join(format!("{name}{extension}"))),
                )
                .collect()
        }
    }
    #[cfg(not(windows))]
    {
        vec![directory.join(name)]
    }
}

pub(crate) fn command_for(path: &Path, args: &[&str]) -> Command {
    #[cfg(windows)]
    {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension == "cmd" || extension == "bat" {
            let mut command = Command::new("cmd.exe");
            command.arg("/D").arg("/S").arg("/C");
            let mut invocation = format!("\"{}\"", path.display());
            for arg in args {
                invocation.push(' ');
                invocation.push_str(arg);
            }
            command.arg(invocation);
            return command;
        }
    }

    let mut command = Command::new(path);
    command.args(args);
    command
}
