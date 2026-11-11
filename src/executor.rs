use crate::app::{AppEvent, PackageManager};
use crate::detector::{command_for, find_binary};
use std::collections::VecDeque;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::mpsc::UnboundedSender;

pub async fn update_selected(items: Vec<PackageManager>, sender: UnboundedSender<AppEvent>) {
    let mut failures = Vec::new();
    let total = items.len();
    for (position, item) in items.into_iter().enumerate() {
        let Some(index) = crate::app::MANAGER_NAMES
            .iter()
            .position(|name| *name == item.name)
        else {
            let error = format!("{}: not in the supported manager list", item.name);
            failures.push(error.clone());
            let _ = sender.send(AppEvent::Log(error));
            continue;
        };
        let _ = sender.send(AppEvent::UpdateStarted {
            index,
            position: position + 1,
            total,
        });
        let _ = sender.send(AppEvent::Log(format!("{}: starting update", item.name)));
        let steps = update_steps(&item.name);
        if steps.is_empty() {
            let error = "no update command is configured".to_owned();
            failures.push(format!("{}: {error}", item.name));
            let _ = sender.send(AppEvent::Log(format!("{}: failed: {error}", item.name)));
            continue;
        }

        let mut manager_failed = false;
        let mut failure_detail = None;
        for (executable, args) in steps {
            let path = if executable == item.name {
                item.path.as_deref().map(Path::new).map(Path::to_path_buf)
            } else {
                find_binary(executable)
            };
            let Some(path) = path else {
                manager_failed = true;
                failure_detail = Some(format!("required command '{executable}' was not found"));
                let _ = sender.send(AppEvent::Log(format!(
                    "{}: failed: {}",
                    item.name,
                    failure_detail.as_deref().unwrap_or_default()
                )));
                break;
            };

            let command_text = format!("{} {}", executable, args.join(" "));
            let _ = sender.send(AppEvent::Log(format!(
                "{}: running {command_text}",
                item.name
            )));

            let result = run_command(&path, &args, &item.name, &sender).await;
            let result = match result {
                Err(error) if item.name == "pnpm" && should_use_corepack(&error) => {
                    let _ = sender.send(AppEvent::Log(
                        "pnpm: Corepack-managed installation detected; retrying with Corepack"
                            .to_owned(),
                    ));
                    match find_binary("corepack") {
                        Some(corepack) => {
                            let args = ["prepare", "pnpm@latest", "--activate"];
                            let _ = sender.send(AppEvent::Log(
                                "pnpm: running corepack prepare pnpm@latest --activate".to_owned(),
                            ));
                            run_command(&corepack, &args, "pnpm", &sender).await
                        }
                        None => Err(format!(
                            "{error}; Corepack was not found on PATH to update this installation"
                        )),
                    }
                }
                result => result,
            };

            match result {
                Ok(()) => {}
                Err(error) => {
                    manager_failed = true;
                    failure_detail = Some(error);
                    let _ = sender.send(AppEvent::Log(format!(
                        "{}: failed: {}",
                        item.name,
                        failure_detail.as_deref().unwrap_or_default()
                    )));
                    break;
                }
            }
        }
        if manager_failed {
            failures.push(format!(
                "{}: {}",
                item.name,
                failure_detail.unwrap_or_else(|| "update failed".to_owned())
            ));
        } else {
            let _ = sender.send(AppEvent::Log(format!("{}: update completed", item.name)));
        }
    }

    let _ = sender.send(AppEvent::UpdateFinished(failures));
}

async fn run_command(
    path: &Path,
    args: &[&str],
    manager: &str,
    sender: &UnboundedSender<AppEvent>,
) -> Result<(), String> {
    let _ = sender.send(AppEvent::Progress(None));
    let mut child = command_for(path, args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("could not start update command: {error}"))?;

    let output = stream_output(&mut child, manager, sender).await;
    if output.is_err() {
        let _ = child.kill().await;
    }
    let status = child
        .wait()
        .await
        .map_err(|error| format!("could not wait for update command: {error}"))?;
    let output = output?;

    if status.success() {
        Ok(())
    } else {
        let output = output.trim();
        if output.is_empty() {
            Err(format!("update command exited with {status}"))
        } else {
            Err(format!("update command exited with {status}: {output}"))
        }
    }
}

async fn stream_output(
    child: &mut Child,
    manager: &str,
    sender: &UnboundedSender<AppEvent>,
) -> Result<String, String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "could not capture command stdout".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "could not capture command stderr".to_owned())?;
    let mut stdout = BufReader::new(stdout).lines();
    let mut stderr = BufReader::new(stderr).lines();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut captured = VecDeque::new();

    while !stdout_done || !stderr_done {
        tokio::select! {
            line = stdout.next_line(), if !stdout_done => {
                match line.map_err(|error| format!("failed reading command stdout: {error}"))? {
                    Some(line) => {
                        if is_progress_output(&line) {
                            let _ = sender.send(AppEvent::Progress(Some(format!("{manager}: {line}"))));
                        } else {
                            let _ = sender.send(AppEvent::Log(format!("{manager}: {line}")));
                            capture_line(&mut captured, line);
                        }
                    }
                    None => stdout_done = true,
                }
            }
            line = stderr.next_line(), if !stderr_done => {
                match line.map_err(|error| format!("failed reading command stderr: {error}"))? {
                    Some(line) => {
                        if is_progress_output(&line) {
                            let _ = sender.send(AppEvent::Progress(Some(format!("{manager}: {line}"))));
                        } else {
                            let _ = sender.send(AppEvent::Log(format!("{manager}: {line}")));
                            capture_line(&mut captured, line);
                        }
                    }
                    None => stderr_done = true,
                }
            }
        }
    }
    Ok(captured.into_iter().collect::<Vec<_>>().join("\n"))
}

fn is_progress_output(line: &str) -> bool {
    let line = line.trim().to_ascii_lowercase();
    [
        "downloading ",
        "downloaded ",
        "installing ",
        "extracting ",
        "resolving ",
        "fetching ",
        "progress:",
    ]
    .iter()
    .any(|prefix| line.contains(prefix))
        || line.contains("% complete")
}

fn capture_line(captured: &mut VecDeque<String>, mut line: String) {
    const MAX_LINES: usize = 100;
    const MAX_LINE_CHARS: usize = 2_000;

    if line.chars().count() > MAX_LINE_CHARS {
        line = line.chars().take(MAX_LINE_CHARS).collect();
    }
    if captured.len() == MAX_LINES {
        captured.pop_front();
    }
    captured.push_back(line);
}

fn should_use_corepack(error: &str) -> bool {
    error.contains("ERR_PNPM_CANT_SELF_UPDATE_IN_COREPACK")
}

fn update_steps(name: &str) -> Vec<(&str, Vec<&str>)> {
    match name {
        "npm" => vec![("npm", vec!["install", "--global", "npm"])],
        "pnpm" => vec![("pnpm", vec!["self-update"])],
        "yarn" => vec![("npm", vec!["install", "--global", "yarn"])],
        "bun" => vec![("bun", vec!["upgrade"])],
        "pip" => vec![("pip", vec!["install", "--upgrade", "pip"])],
        "pipx" => vec![("pip", vec!["install", "--user", "--upgrade", "pipx"])],
        "poetry" => vec![("poetry", vec!["self", "update"])],
        "uv" => vec![("uv", vec!["self", "update"])],
        "cargo" => vec![("rustup", vec!["update", "stable"])],
        "rustup" => vec![("rustup", vec!["update", "stable"])],
        "flutter" => vec![("flutter", vec!["upgrade"])],
        "brew" => vec![("brew", vec!["update"]), ("brew", vec!["upgrade"])],
        "winget" => vec![(
            "winget",
            vec![
                "upgrade",
                "--all",
                "--silent",
                "--accept-source-agreements",
                "--accept-package-agreements",
            ],
        )],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_progress_output, should_use_corepack, update_steps};

    #[test]
    fn system_package_manager_updates_in_order() {
        let steps = update_steps("brew");

        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0], ("brew", vec!["update"]));
        assert_eq!(steps[1], ("brew", vec!["upgrade"]));
    }

    #[test]
    fn unsupported_manager_has_no_update_command() {
        assert!(update_steps("unknown").is_empty());
    }

    #[test]
    fn corepack_fallback_requires_the_specific_pnpm_error() {
        assert!(should_use_corepack("ERR_PNPM_CANT_SELF_UPDATE_IN_COREPACK"));
        assert!(!should_use_corepack("permission denied"));
    }

    #[test]
    fn recognizes_download_progress_lines() {
        assert!(is_progress_output("bun: Downloading [51.61MiB]"));
        assert!(is_progress_output("Progress: 42% complete"));
        assert!(!is_progress_output("ERR_PNPM_CANT_SELF_UPDATE_IN_COREPACK"));
    }
}
