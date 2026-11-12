use crate::app::{AppEvent, PackageManager};
use crate::detector::{command_for, find_binary};
use crate::npm_compat::{current_node_version, latest_compatible_npm, npm_registry_client};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
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
        if item.name == "npm" {
            match update_npm(&item, &sender).await {
                Ok(()) => {
                    let _ = sender.send(AppEvent::Log("npm: update completed".to_owned()));
                }
                Err(error) => {
                    failures.push(format!("npm: {error}"));
                    let _ = sender.send(AppEvent::Log(format!("npm: failed: {error}")));
                }
            }
            continue;
        }

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

#[derive(Debug, PartialEq, Eq)]
enum NpmInstallSource {
    Nvm { root: PathBuf, node_version: String },
    Homebrew,
    Volta,
    SystemManaged,
    NpmManaged,
}

fn npm_install_source(path: &Path) -> NpmInstallSource {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(source) = nvm_install_source(path).or_else(|| nvm_install_source(&resolved)) {
        return source;
    }

    let path_text = resolved.to_string_lossy().to_ascii_lowercase();
    if path_text.contains("/cellar/node/") || path_text.contains("/homebrew/cellar/node/") {
        NpmInstallSource::Homebrew
    } else if path_text.contains("/.volta/") || path_text.contains("/volta/bin/") {
        NpmInstallSource::Volta
    } else if resolved.starts_with("/usr/bin") {
        NpmInstallSource::SystemManaged
    } else {
        NpmInstallSource::NpmManaged
    }
}

fn nvm_install_source(path: &Path) -> Option<NpmInstallSource> {
    let version_dir = path.ancestors().find(|ancestor| {
        ancestor
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "node")
    })?;
    let node_version = version_dir.file_name()?.to_str()?.to_owned();
    let root = version_dir
        .ancestors()
        .find(|ancestor| ancestor.join("nvm.sh").is_file())?;
    Some(NpmInstallSource::Nvm {
        root: root.to_path_buf(),
        node_version,
    })
}

async fn update_npm(
    item: &PackageManager,
    sender: &UnboundedSender<AppEvent>,
) -> Result<(), String> {
    let npm_path = item
        .path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| "detected npm has no executable path".to_owned())?;
    let node_version = current_node_version().await?;
    let client = npm_registry_client()?;
    let target_version = latest_compatible_npm(&client, &node_version).await?;
    let target = target_version.to_string();
    let _ = sender.send(AppEvent::Log(format!(
        "npm: latest version compatible with Node.js {node_version} is {target}"
    )));

    let update_result = match npm_install_source(&npm_path) {
        NpmInstallSource::Nvm { root, node_version } => {
            let root = root.to_str().ok_or_else(|| {
                format!("NVM root path is not valid UTF-8: {}", root.display())
            })?;
            let _ = sender.send(AppEvent::Log(format!(
                "npm: detected NVM; updating npm for Node {node_version}"
            )));
            let script = ". \"$1/nvm.sh\" && nvm use \"$2\" && nvm install-latest-npm";
            let mut command = tokio::process::Command::new("bash");
            command
                .arg("-c")
                .arg(script)
                .arg("clim")
                .arg(root)
                .arg(&node_version);
            run_command_builder(command, "npm", sender).await
        }
        NpmInstallSource::Homebrew => {
            let brew = find_binary("brew").ok_or_else(|| {
                "npm appears to be Homebrew-managed, but `brew` was not found on PATH".to_owned()
            })?;
            let _ = sender.send(AppEvent::Log(
                "npm: detected Homebrew; upgrading the Node formula (includes npm)".to_owned(),
            ));
            let update_command = command_for(&brew, &["update"]);
            run_command_builder(update_command, "npm", sender).await?;
            let upgrade_command = command_for(&brew, &["upgrade", "node"]);
            run_command_builder(upgrade_command, "npm", sender).await
        }
        NpmInstallSource::Volta => {
            let volta = find_binary("volta").ok_or_else(|| {
                "npm appears to be Volta-managed, but `volta` was not found on PATH".to_owned()
            })?;
            let _ = sender.send(AppEvent::Log(
                "npm: detected Volta; installing its latest npm tool".to_owned(),
            ));
            let package = format!("npm@{target}");
            let command = command_for(&volta, &["install", &package]);
            run_command_builder(command, "npm", sender).await
        }
        NpmInstallSource::SystemManaged => Err(
            "npm is installed under /usr/bin and appears to be owned by the operating system. Update it with your OS package manager (for example, `sudo apt upgrade npm`); clim will not replace OS-managed files directly."
                .to_owned(),
        ),
        NpmInstallSource::NpmManaged => {
            let _ = sender.send(AppEvent::Log(
                "npm: updating the npm package in the active Node installation".to_owned(),
            ));
            let package = format!("npm@{target}");
            let command = command_for(&npm_path, &["install", "--global", &package]);
            run_command_builder(command, "npm", sender).await
        }
    };
    update_result?;
    let installed = verify_npm_version(&npm_path, &target_version).await?;
    let _ = sender.send(AppEvent::Log(format!(
        "npm: verified installed version {installed} (Node.js {node_version})"
    )));
    Ok(())
}

async fn verify_npm_version(
    npm_path: &Path,
    target: &semver::Version,
) -> Result<semver::Version, String> {
    let mut command = command_for(npm_path, &["--version"]);
    command.kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(15), command.output())
        .await
        .map_err(|_| "timed out verifying npm version after update".to_owned())?
        .map_err(|error| format!("could not verify npm version after update: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "npm version verification exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let output = String::from_utf8_lossy(&output.stdout);
    let installed = semver::Version::parse(output.trim().trim_start_matches('v'))
        .map_err(|error| format!("could not parse installed npm version '{output}': {error}"))?;
    if installed < *target {
        return Err(format!(
            "update command completed but npm is still {installed}; {target} is the latest compatible version for the active Node.js"
        ));
    }
    Ok(installed)
}

async fn run_command_builder(
    mut command: tokio::process::Command,
    manager: &str,
    sender: &UnboundedSender<AppEvent>,
) -> Result<(), String> {
    let _ = sender.send(AppEvent::Progress(None));
    let mut child = command
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
    } else if output.trim().is_empty() {
        Err(format!("update command exited with {status}"))
    } else {
        Err(format!(
            "update command exited with {status}: {}",
            output.trim()
        ))
    }
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
    use super::{
        is_progress_output, npm_install_source, should_use_corepack, update_steps, NpmInstallSource,
    };
    use std::path::{Path, PathBuf};

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

    #[test]
    fn identifies_nvm_managed_npm_installation() {
        let root = std::env::temp_dir().join("clim-test-nvm");
        let nvm_script = root.join("nvm.sh");
        std::fs::create_dir_all(&root).expect("create test NVM root");
        std::fs::write(&nvm_script, "").expect("create test nvm.sh");
        let npm = root.join("versions/node/v22.1.0/bin/npm");
        std::fs::create_dir_all(npm.parent().expect("npm parent")).expect("create test bin");

        assert_eq!(
            npm_install_source(&npm),
            NpmInstallSource::Nvm {
                root: PathBuf::from(&root),
                node_version: "v22.1.0".to_owned(),
            }
        );

        std::fs::remove_file(nvm_script).expect("remove test nvm.sh");
        std::fs::remove_dir_all(root).expect("remove test NVM root");
    }

    #[test]
    fn recognizes_brew_and_volta_npm_paths() {
        assert_eq!(
            npm_install_source(Path::new("/opt/homebrew/Cellar/node/22.0.0/bin/npm")),
            NpmInstallSource::Homebrew
        );
        assert_eq!(
            npm_install_source(Path::new("/Users/test/.volta/bin/npm")),
            NpmInstallSource::Volta
        );
    }

    #[test]
    fn system_npm_installations_are_not_replaced_directly() {
        assert_eq!(
            npm_install_source(Path::new("/usr/bin/npm")),
            NpmInstallSource::SystemManaged
        );
        assert_eq!(
            npm_install_source(Path::new("/usr/local/bin/npm")),
            NpmInstallSource::NpmManaged
        );
    }
}
