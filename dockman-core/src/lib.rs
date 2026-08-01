//! dockman-core: shared types. No I/O here — the GUI backend does the
//! actual ssh/docker-compose work; this crate just defines the data
//! both the backend and (indirectly, via serde) the frontend agree on.

use serde::{Deserialize, Serialize};

/// A remote (or local) machine, identified by its alias in ~/.ssh/config.
/// Auth, ProxyJump, port, identity file etc. are all whatever that alias
/// already resolves to — dockman doesn't manage any of that itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostConfig {
    /// Unique id, currently just the ssh alias.
    pub id: String,
    /// The Host entry in ~/.ssh/config, e.g. "cachyos-box".
    pub ssh_alias: String,
    /// Friendly display name; defaults to ssh_alias.
    pub label: String,
    pub compose_dirs: Vec<ComposeDir>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComposeDir {
    /// Unique id, currently just the path.
    pub id: String,
    /// Absolute path on the remote host containing docker-compose.yml.
    pub path: String,
    /// Friendly display name; defaults to the last path segment.
    pub label: String,
}

/// One service/container as reported by `docker compose ps --format json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeService {
    pub name: String,
    pub service: String,
    pub image: String,
    /// e.g. "running", "exited"
    pub state: String,
    /// human readable, e.g. "Up 3 hours (healthy)"
    pub status: String,
    pub health: Option<String>,
    pub ports: String,
}

/// Result of listing a stack: either we got service data, or the ssh/
/// docker-compose call itself failed (host down, dir wrong, etc.) and
/// we surface that instead of pretending the stack is empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackStatus {
    pub host_id: String,
    pub dir_id: String,
    pub services: Vec<ComposeService>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StackAction {
    /// docker compose pull && docker compose up -d
    Update,
    /// docker compose stop
    Stop,
    /// docker compose restart
    Restart,
    /// docker compose up -d (start without pulling)
    Start,
}

impl StackAction {
    /// The docker compose subcommand(s) to run in the stack directory,
    /// chained with && so any failure stops the sequence.
    pub fn compose_commands(&self) -> &'static [&'static str] {
        match self {
            StackAction::Update => &["compose pull", "compose up -d"],
            StackAction::Stop => &["compose stop"],
            StackAction::Restart => &["compose restart"],
            StackAction::Start => &["compose up -d"],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionOutcome {
    pub host_id: String,
    pub dir_id: String,
    pub ok: bool,
    /// Combined stdout+stderr, trimmed, for display in the UI on failure
    /// (or on success if you want to see it — the frontend decides).
    pub output: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VersionStatus {
    UpToDate,
    UpdateAvailable,
    /// Couldn't determine — buildx missing on that host, image built
    /// locally with no registry digest, private registry auth issue,
    /// network hiccup, etc.
    Unknown,
}

/// Result of comparing a running image's digest against what's
/// currently on the registry for the same tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageVersionInfo {
    pub image: String,
    pub status: VersionStatus,
    pub running_digest: Option<String>,
    pub latest_digest: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_chains_pull_then_up() {
        assert_eq!(
            StackAction::Update.compose_commands(),
            &["compose pull", "compose up -d"]
        );
    }

    #[test]
    fn status_serializes_snake_case() {
        let s = serde_json::to_string(&VersionStatus::UpdateAvailable).unwrap();
        assert_eq!(s, "\"update_available\"");
    }
}
