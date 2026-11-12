use crate::app::{AppEvent, PackageManager, UpdateAvailability};
use crate::detector::{command_for, find_binary};
use crate::npm_compat::{current_node_version, latest_compatible_npm};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinSet;
use tokio::time::timeout;

pub async fn check_updates(
    managers: Vec<(usize, PackageManager)>,
    sender: UnboundedSender<AppEvent>,
) {
    let _ = sender.send(AppEvent::UpdateCheckStarted);
    let (client, client_error) = match reqwest::Client::builder()
        .user_agent(concat!("clim/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
    {
        Ok(client) => (Some(client), None),
        Err(error) => (None, Some(error.to_string())),
    };

    let mut tasks = JoinSet::new();
    for (index, manager) in managers {
        let client = client.clone();
        let client_error = client_error.clone();
        tasks.spawn(async move {
            let result = match client {
                Some(client) => check_one(&client, &manager).await,
                None => UpdateAvailability::Failed(format!(
                    "could not initialize update checker: {}",
                    client_error.unwrap_or_else(|| "unknown client error".to_owned())
                )),
            };
            (index, manager.name, result)
        });
    }

    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((index, name, result)) => {
                if matches!(
                    result,
                    UpdateAvailability::Failed(_) | UpdateAvailability::Unavailable(_)
                ) {
                    let message = match &result {
                        UpdateAvailability::Failed(message)
                        | UpdateAvailability::Unavailable(message) => {
                            Some(format!("{name}: update check: {message}"))
                        }
                        _ => None,
                    };
                    if let Some(message) = message {
                        let _ = sender.send(AppEvent::Log(message));
                    }
                }
                let _ = sender.send(AppEvent::UpdateCheckResult { index, result });
            }
            Err(error) => {
                let _ = sender.send(AppEvent::Log(format!("update check task failed: {error}")));
            }
        }
    }
    let _ = sender.send(AppEvent::UpdateCheckFinished);
}

async fn check_one(client: &reqwest::Client, manager: &PackageManager) -> UpdateAvailability {
    if matches!(manager.name.as_str(), "cargo" | "rustup") {
        return check_rustup(manager).await;
    }
    if manager.name == "winget" {
        return UpdateAvailability::Unavailable(
            "WinGet updates installed applications rather than itself; run `winget upgrade` to review available application updates.".to_owned(),
        );
    }
    if manager.name == "npm" {
        return check_npm(client, manager).await;
    }

    let current = manager.version.as_deref().and_then(parse_version);
    let latest_result = match manager.name.as_str() {
        "pnpm" | "yarn" => {
            fetch_version(
                client,
                &format!("https://registry.npmjs.org/{}/latest", manager.name),
            )
            .await
        }
        "bun" => fetch_github_tag(client, "oven-sh/bun").await,
        "pip" | "pipx" | "poetry" | "uv" => {
            fetch_version(
                client,
                &format!("https://pypi.org/pypi/{}/json", manager.name),
            )
            .await
        }
        "flutter" => fetch_flutter_version(client).await,
        "brew" => fetch_github_tag(client, "Homebrew/brew").await,
        _ => {
            return UpdateAvailability::Unavailable(
                "automatic version checks are not configured for this manager".to_owned(),
            )
        }
    };

    match latest_result {
        Ok(latest) => match current {
            Some(current) if compare_versions(&current, &latest).is_some() => {
                if compare_versions(&current, &latest).unwrap_or(std::cmp::Ordering::Equal)
                    == std::cmp::Ordering::Less
                {
                    UpdateAvailability::Available { current, latest }
                } else {
                    UpdateAvailability::UpToDate { current, latest }
                }
            }
            Some(current) => UpdateAvailability::Unavailable(format!(
                "could not compare installed version '{current}' with latest '{latest}'"
            )),
            None => UpdateAvailability::Unavailable(format!(
                "latest version is {latest}, but the installed version could not be parsed"
            )),
        },
        Err(error) => UpdateAvailability::Failed(error),
    }
}

async fn check_npm(client: &reqwest::Client, manager: &PackageManager) -> UpdateAvailability {
    let node_version = match current_node_version().await {
        Ok(version) => version,
        Err(error) => return UpdateAvailability::Failed(error),
    };
    let latest = match latest_compatible_npm(client, &node_version).await {
        Ok(version) => version.to_string(),
        Err(error) => return UpdateAvailability::Failed(error),
    };
    let Some(current) = manager.version.as_deref().and_then(parse_version) else {
        return UpdateAvailability::Unavailable(format!(
            "could not parse installed npm version; latest compatible with Node.js {node_version} is {latest}"
        ));
    };

    match compare_versions(&current, &latest) {
        Some(std::cmp::Ordering::Less) => UpdateAvailability::Available { current, latest },
        Some(_) => UpdateAvailability::UpToDate { current, latest },
        None => UpdateAvailability::Unavailable(format!(
            "could not compare npm {current} with compatible version {latest} for Node.js {node_version}"
        )),
    }
}

async fn fetch_version(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let payload: Value = response.json().await.map_err(|error| error.to_string())?;
    let version = payload
        .get("version")
        .or_else(|| payload.pointer("/dist-tags/latest"))
        .or_else(|| payload.pointer("/info/version"))
        .and_then(Value::as_str)
        .ok_or_else(|| "registry response did not contain a latest version".to_owned())?;
    Ok(version.to_owned())
}

async fn fetch_flutter_version(client: &reqwest::Client) -> Result<String, String> {
    let platform = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        return Err("unsupported platform for Flutter release checks".to_owned());
    };
    let url = format!(
        "https://storage.googleapis.com/flutter_infra_release/releases/releases_{platform}.json"
    );
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let payload: Value = response.json().await.map_err(|error| error.to_string())?;
    let stable_hash = payload
        .pointer("/current_release/stable")
        .and_then(Value::as_str)
        .ok_or_else(|| "Flutter release metadata has no stable release".to_owned())?;
    payload
        .get("releases")
        .and_then(Value::as_array)
        .and_then(|releases| {
            releases
                .iter()
                .find(|release| release.get("hash").and_then(Value::as_str) == Some(stable_hash))
        })
        .and_then(|release| release.get("version"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Flutter stable version was not found in release metadata".to_owned())
}

async fn fetch_github_tag(client: &reqwest::Client, repository: &str) -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{repository}/releases/latest");
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let payload: Value = response.json().await.map_err(|error| error.to_string())?;
    payload
        .get("tag_name")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "release response did not contain a version tag".to_owned())
}

async fn check_rustup(manager: &PackageManager) -> UpdateAvailability {
    let path =
        find_binary("rustup").or_else(|| manager.path.as_deref().map(std::path::PathBuf::from));
    let Some(path) = path else {
        return UpdateAvailability::Unavailable("rustup was not found on PATH".to_owned());
    };
    let mut command = command_for(&path, &["check"]);
    command.kill_on_drop(true);
    let output = match timeout(Duration::from_secs(30), command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return UpdateAvailability::Failed(format!("could not run `rustup check`: {error}"))
        }
        Err(_) => {
            return UpdateAvailability::Failed("`rustup check` timed out".to_owned());
        }
    };
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return UpdateAvailability::Failed(format!(
            "`rustup check` exited with {}: {}",
            output.status,
            text.trim()
        ));
    }

    let status_line = text.lines().find(|line| {
        let line = line.to_ascii_lowercase();
        line.contains("stable")
            && (line.contains("update available") || line.contains("up to date"))
    });
    match status_line {
        Some(line) if line.to_ascii_lowercase().contains("update available") => {
            let (current, latest) = line
                .rsplit_once(':')
                .and_then(|(_, versions)| versions.split_once("->"))
                .map(|(current, latest)| (current.trim(), latest.trim()))
                .unwrap_or(("installed stable", "new stable"));
            UpdateAvailability::Available {
                current: current.to_owned(),
                latest: latest.to_owned(),
            }
        }
        Some(line) if line.to_ascii_lowercase().contains("up to date") => {
            let current = line
                .rsplit_once(':')
                .map(|(_, version)| version.trim())
                .filter(|version| !version.is_empty())
                .unwrap_or("stable toolchain");
            UpdateAvailability::UpToDate {
                current: current.to_owned(),
                latest: current.to_owned(),
            }
        }
        _ => UpdateAvailability::Unavailable(format!(
            "could not determine stable toolchain status: {}",
            text.trim()
        )),
    }
}

fn parse_version(input: &str) -> Option<String> {
    let input = input.trim();
    let start = input.find(|character: char| character.is_ascii_digit())?;
    let version: String = input[start..]
        .chars()
        .take_while(|character| {
            character.is_ascii_alphanumeric() || matches!(*character, '.' | '-' | '+')
        })
        .collect();
    let version = version.trim_end_matches('.');
    if version.is_empty() {
        return None;
    }

    let version = version.trim_start_matches('v');
    if version.is_empty() {
        return None;
    }

    Some(version.to_owned())
}

fn compare_versions(current: &str, latest: &str) -> Option<std::cmp::Ordering> {
    let current = parse_version(current)?;
    let latest = parse_version(latest)?;
    let current = normalize_semver(&current);
    let latest = normalize_semver(&latest);
    let current = semver::Version::parse(&current).ok()?;
    let latest = semver::Version::parse(&latest).ok()?;
    current.partial_cmp(&latest)
}

fn normalize_semver(version: &str) -> String {
    let version = version.trim();
    let (core, suffix) = match version.split_once('+') {
        Some((core, suffix)) => (core, Some(format!("+{suffix}"))),
        None => (version, None),
    };
    let (core, prerelease) = match core.split_once('-') {
        Some((core, prerelease)) => (core, Some(format!("-{prerelease}"))),
        None => (core, None),
    };
    let mut parts = core.split('.').filter(|part| !part.is_empty());
    let major = parts.next().unwrap_or("0");
    let minor = parts.next().unwrap_or("0");
    let patch = parts.next().unwrap_or("0");
    let mut normalized = format!("{major}.{minor}.{patch}");
    if let Some(prerelease) = prerelease {
        normalized.push_str(&prerelease);
    }
    if let Some(suffix) = suffix {
        normalized.push_str(&suffix);
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::{compare_versions, parse_version};
    use std::cmp::Ordering;

    #[test]
    fn extracts_versions_from_manager_output() {
        assert_eq!(parse_version("npm 10.2.0"), Some("10.2.0".to_owned()));
        assert_eq!(
            parse_version("pip 26.2 from /usr/lib/python/site-packages"),
            Some("26.2".to_owned())
        );
        assert_eq!(parse_version("no version"), None);
    }

    #[test]
    fn compares_numeric_versions_without_lexicographic_errors() {
        assert_eq!(compare_versions("1.9.0", "1.10.0"), Some(Ordering::Less));
        assert_eq!(compare_versions("2.0", "2.0.0"), Some(Ordering::Equal));
        assert_eq!(compare_versions("3.1.0", "3.0.9"), Some(Ordering::Greater));
    }

    #[test]
    fn compares_semver_prerelease_versions_correctly() {
        assert_eq!(
            compare_versions("1.0.0-beta.2", "1.0.0-beta.10"),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_versions("Node.js v22.1.0", "22.1.0"),
            Some(Ordering::Equal)
        );
    }
}
