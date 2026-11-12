use crate::detector::{command_for, find_binary};
use serde_json::Value;
use std::time::Duration;
use tokio::time::timeout;

pub async fn current_node_version() -> Result<semver::Version, String> {
    let path = find_binary("node").ok_or_else(|| "Node.js was not found on PATH".to_owned())?;
    let mut command = command_for(&path, &["--version"]);
    command.kill_on_drop(true);
    let output = timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| "running `node --version` timed out".to_owned())?
        .map_err(|error| format!("could not run `node --version`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`node --version` exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let version = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_start_matches('v')
        .to_owned();
    semver::Version::parse(&version)
        .map_err(|error| format!("could not parse Node.js version '{version}': {error}"))
}

pub async fn latest_compatible_npm(
    client: &reqwest::Client,
    node_version: &semver::Version,
) -> Result<semver::Version, String> {
    let response = client
        .get("https://registry.npmjs.org/npm")
        .send()
        .await
        .map_err(|error| format!("could not query the npm registry: {error}"))?
        .error_for_status()
        .map_err(|error| format!("npm registry request failed: {error}"))?;
    let metadata: Value = response
        .json()
        .await
        .map_err(|error| format!("could not parse npm registry metadata: {error}"))?;
    latest_compatible_npm_from_metadata(&metadata, node_version)
}

fn latest_compatible_npm_from_metadata(
    metadata: &Value,
    node_version: &semver::Version,
) -> Result<semver::Version, String> {
    let versions = metadata
        .get("versions")
        .and_then(Value::as_object)
        .ok_or_else(|| "npm registry metadata did not contain package versions".to_owned())?;

    versions
        .iter()
        .filter_map(|(version_text, metadata)| {
            let version = semver::Version::parse(version_text).ok()?;
            if !version.pre.is_empty() {
                return None;
            }
            let node_requirement = metadata
                .pointer("/engines/node")
                .and_then(Value::as_str)
                .unwrap_or("*");
            npm_node_requirement_matches(node_requirement, node_version).then_some(version)
        })
        .max()
        .ok_or_else(|| {
            format!("npm registry has no stable version compatible with Node.js {node_version}")
        })
}

fn npm_node_requirement_matches(range: &str, version: &semver::Version) -> bool {
    let range = range.trim();
    if range.is_empty() || range == "*" {
        return true;
    }

    range.split("||").any(|alternative| {
        let alternative = alternative.trim();
        if alternative.is_empty() || alternative == "*" {
            return true;
        }
        semver::VersionReq::parse(alternative).is_ok_and(|requirement| requirement.matches(version))
    })
}

pub fn npm_registry_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("clim/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("could not initialize npm registry client: {error}"))
}

#[cfg(test)]
mod tests {
    use super::latest_compatible_npm_from_metadata;
    use semver::Version;
    use serde_json::json;

    #[test]
    fn npm_node_engine_range_excludes_node_25() {
        assert!(!super::npm_node_requirement_matches(
            "^22.22.2 || ^24.15.0 || >=26.0.0",
            &Version::parse("25.4.0").unwrap()
        ));
        assert!(super::npm_node_requirement_matches(
            "^22.22.2 || ^24.15.0 || >=26.0.0",
            &Version::parse("26.0.0").unwrap()
        ));
    }

    #[test]
    fn selects_highest_npm_release_compatible_with_active_node() {
        let metadata = json!({
            "versions": {
                "11.21.2": { "engines": { "node": "^20.17.0 || >=22.9.0" } },
                "12.2.2": { "engines": { "node": "^22.22.2 || ^24.15.0 || >=26.0.0" } }
            }
        });

        assert_eq!(
            latest_compatible_npm_from_metadata(&metadata, &Version::parse("25.4.0").unwrap())
                .unwrap(),
            Version::parse("11.21.2").unwrap()
        );
        assert_eq!(
            latest_compatible_npm_from_metadata(&metadata, &Version::parse("26.0.0").unwrap())
                .unwrap(),
            Version::parse("12.2.2").unwrap()
        );
    }

    #[test]
    fn wildcard_or_empty_node_ranges_match_every_supported_node() {
        assert!(super::npm_node_requirement_matches(
            "*",
            &Version::parse("25.0.0").unwrap()
        ));
        assert!(super::npm_node_requirement_matches(
            "  *  ",
            &Version::parse("25.0.0").unwrap()
        ));
        assert!(super::npm_node_requirement_matches(
            "^22.0.0 || *",
            &Version::parse("25.0.0").unwrap()
        ));
    }
}
