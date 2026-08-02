use dockman_core::{
    ActionOutcome, ComposeDir, ComposeService, HostConfig, ImageVersionInfo, MaintenanceAction,
    MaintenanceOutcome, StackAction, StackStatus, VersionStatus,
};
use serde::Deserialize;
use tokio::process::Command;

/// Run `ssh <alias> "<remote_command>"` and return (success, combined
/// stdout+stderr). BatchMode=yes means it fails fast instead of hanging
/// on a password prompt if key auth isn't set up for that alias.
async fn run_ssh(alias: &str, remote_command: &str) -> anyhow::Result<(bool, String)> {
    let output = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg(alias)
        .arg(remote_command)
        .output()
        .await?;

    let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }
    Ok((output.status.success(), combined.trim().to_string()))
}

/// Single-quote a path for the remote shell, escaping any embedded
/// single quotes. Handles spaces in directory names correctly.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn cd_prefix(dir: &str) -> String {
    format!("cd {}", shell_quote(dir))
}

/// `ssh <alias> "echo ok"` — a quick reachability check for the
/// "Add host" dialog, distinct from failing partway through an action.
pub async fn check_host(alias: &str) -> anyhow::Result<(bool, String)> {
    run_ssh(alias, "echo ok").await
}

pub async fn fetch_stack_status(host: &HostConfig, dir: &ComposeDir) -> StackStatus {
    let remote_cmd = format!("{} && docker compose ps --format json", cd_prefix(&dir.path));
    match run_ssh(&host.ssh_alias, &remote_cmd).await {
        Ok((true, output)) => {
            let services = output
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<RawService>(line).ok())
                .map(RawService::into_service)
                .collect();
            StackStatus {
                host_id: host.id.clone(),
                dir_id: dir.id.clone(),
                services,
                error: None,
            }
        }
        Ok((false, output)) => StackStatus {
            host_id: host.id.clone(),
            dir_id: dir.id.clone(),
            services: Vec::new(),
            error: Some(if output.is_empty() {
                "ssh or docker compose command failed".to_string()
            } else {
                output
            }),
        },
        Err(e) => StackStatus {
            host_id: host.id.clone(),
            dir_id: dir.id.clone(),
            services: Vec::new(),
            error: Some(e.to_string()),
        },
    }
}

pub async fn run_action(host: &HostConfig, dir: &ComposeDir, action: StackAction) -> ActionOutcome {
    let chained = action
        .compose_commands()
        .iter()
        .map(|c| format!("docker {c}"))
        .collect::<Vec<_>>()
        .join(" && ");
    let remote_cmd = format!("{} && {chained}", cd_prefix(&dir.path));

    match run_ssh(&host.ssh_alias, &remote_cmd).await {
        Ok((ok, output)) => ActionOutcome {
            host_id: host.id.clone(),
            dir_id: dir.id.clone(),
            ok,
            output,
        },
        Err(e) => ActionOutcome {
            host_id: host.id.clone(),
            dir_id: dir.id.clone(),
            ok: false,
            output: e.to_string(),
        },
    }
}

/// Host-wide Docker cleanup (image/container/volume/build-cache/system
/// prune) — no compose directory involved, this runs directly on the
/// daemon. No `cd` needed since these commands aren't scoped to a
/// working directory.
pub async fn run_maintenance(host: &HostConfig, action: MaintenanceAction) -> MaintenanceOutcome {
    let remote_cmd = format!("docker {}", action.command());
    match run_ssh(&host.ssh_alias, &remote_cmd).await {
        Ok((ok, output)) => MaintenanceOutcome {
            host_id: host.id.clone(),
            action,
            ok,
            output,
        },
        Err(e) => MaintenanceOutcome {
            host_id: host.id.clone(),
            action,
            ok: false,
            output: e.to_string(),
        },
    }
}

/// Compare the digest of the image currently running against what the
/// registry has for the same floating tag right now, without pulling
/// the full image — just its manifest. Runs entirely on the remote
/// host (including the hashing) so no binary manifest bytes ever have
/// to survive a round trip through our UTF-8 ssh-output pipeline.
///
/// Requires `docker buildx` on the remote host (bundled with modern
/// Docker Engine / Docker Desktop, but worth confirming on distros
/// that package things separately).
pub async fn check_image_version(host: &HostConfig, image: &str) -> ImageVersionInfo {
    let parsed = parse_image_ref(image);

    // If the compose file pins an exact digest (repo:tag@sha256:...),
    // that digest *is* the running version — no need to ask the local
    // daemon. What's actually interesting is whether that pin still
    // matches what the floating tag resolves to on the registry now,
    // so the registry lookup below always targets repo:tag, never the
    // pin itself (checking a pinned digest against itself would be a
    // trivial, useless "match").
    let embedded_digest = parsed.digest.map(str::to_string);
    let registry_ref = match parsed.tag {
        Some(tag) => format!("{}:{}", parsed.repo, tag),
        None => parsed.repo.to_string(),
    };

    let script = version_check_script(image, &registry_ref);
    let (_ok, output) = match run_ssh(&host.ssh_alias, &script).await {
        Ok(v) => v,
        Err(_) => {
            return ImageVersionInfo {
                image: image.to_string(),
                status: VersionStatus::Unknown,
                running_digest: embedded_digest,
                latest_digest: None,
            }
        }
    };

    let mut parts = output.splitn(2, "---DOCKMAN-SPLIT---");
    let local_json = parts.next().unwrap_or("[]").trim();
    let remote_hash = parts.next().unwrap_or("").trim();

    let running_digest = embedded_digest.or_else(|| extract_matching_digest(local_json, image));
    let latest_digest = if is_sha256_hex(remote_hash) {
        Some(format!("sha256:{remote_hash}"))
    } else {
        None
    };

    let status = match (&running_digest, &latest_digest) {
        (Some(a), Some(b)) if a == b => VersionStatus::UpToDate,
        (Some(_), Some(_)) => VersionStatus::UpdateAvailable,
        _ => VersionStatus::Unknown,
    };

    ImageVersionInfo {
        image: image.to_string(),
        status,
        running_digest,
        latest_digest,
    }
}

struct ParsedImageRef<'a> {
    repo: &'a str,
    tag: Option<&'a str>,
    digest: Option<&'a str>,
}

/// Parses `[registry[:port]/]repo[:tag][@sha256:digest]`. The digest
/// suffix trips up naive tag-extraction (a plain "split on the last
/// colon" grabs the hex digest instead of the tag) since it contains
/// its own colon — so digest is stripped first, then tag is extracted
/// from what's left.
fn parse_image_ref(image: &str) -> ParsedImageRef<'_> {
    let (name_and_tag, digest) = match image.rsplit_once('@') {
        Some((left, right)) if right.starts_with("sha256:") => (left, Some(right)),
        _ => (image, None),
    };
    let (repo, tag) = match name_and_tag.rsplit_once(':') {
        Some((r, t)) if !t.contains('/') => (r, Some(t)),
        _ => (name_and_tag, None),
    };
    ParsedImageRef { repo, tag, digest }
}

/// Double-quote a value for use *inside* a bash script we're already
/// wrapping in single quotes at the outer (ssh) layer — see
/// `version_check_script`. Docker image references only use a small,
/// safe character set (alnum, `. - _ / : @`), so this is defensive
/// rather than strictly necessary.
fn bash_double_quote(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`");
    format!("\"{escaped}\"")
}

/// Builds a small remote script that:
/// 1. reads the locally-cached image's RepoDigests (free, no network)
/// 2. fetches just the registry manifest for `registry_ref` — the
///    floating repo:tag, deliberately never a digest-pinned reference,
///    since checking a pin against itself would trivially "match" —
///    and hashes it (kilobytes, not the full image)
/// 3. prints both, separated by a marker, for us to split on
///
/// This needs bash-specific syntax (`VAR=$(...)`, `||` fallbacks),
/// which isn't valid fish (or some other login shells). ssh always
/// runs whatever we send through the target account's *login* shell
/// first, so we can't assume that's bash — instead we wrap the whole
/// script in `bash -c '...'`, using only single-quote literal
/// grouping at the outer layer, which every shell (fish included)
/// treats the same way. Everything bash-specific lives *inside* that
/// single-quoted string, safe from the outer shell's parser as long
/// as it contains no literal single quotes itself — which is why the
/// inner script uses double quotes throughout instead.
///
/// Uses `sha256sum` where available (most Linux) and falls back to
/// `shasum -a 256` (macOS, and Linux systems without coreutils'
/// sha256sum) so this works on both your CachyOS box and the Mac Mini.
fn version_check_script(image: &str, registry_ref: &str) -> String {
    let img = bash_double_quote(image);
    let reg_ref = bash_double_quote(registry_ref);
    let mut inner = String::new();
    inner.push_str("LOCAL=$(docker image inspect ");
    inner.push_str(&img);
    inner.push_str(" --format \"{{json .RepoDigests}}\" 2>/dev/null || echo \"[]\"); ");
    inner.push_str("REMOTE_HASH=$(docker buildx imagetools inspect ");
    inner.push_str(&reg_ref);
    inner.push_str(
        " --raw 2>/dev/null | (sha256sum 2>/dev/null || shasum -a 256) | cut -d\" \" -f1); ",
    );
    inner.push_str("printf \"%s\\n---DOCKMAN-SPLIT---\\n%s\\n\" \"$LOCAL\" \"$REMOTE_HASH\"");

    // shell_quote wraps `inner` in single quotes; safe here because
    // `inner` (built entirely with double quotes above) never
    // contains a literal single-quote character.
    format!("bash -c {}", shell_quote(&inner))
}

/// `docker image inspect --format '{{json .RepoDigests}}'` returns
/// entries like `"myrepo/app@sha256:abcd..."`. Find the one matching
/// this image's repo (ignoring tag) and return just the digest part.
///
/// Docker stores official single-word images (postgres, redis, ...)
/// internally under an implicit `docker.io/library/` prefix even
/// though a compose file just says `postgres:16` — so both sides get
/// normalized before comparing, or these would always mismatch and
/// show as "unknown" despite being perfectly checkable.
fn extract_matching_digest(repo_digests_json: &str, image: &str) -> Option<String> {
    let target_repo = normalize_docker_repo(image_repo(image));
    let entries: Vec<String> = serde_json::from_str(repo_digests_json).ok()?;
    entries.into_iter().find_map(|entry| {
        let (entry_repo, digest) = entry.split_once('@')?;
        (normalize_docker_repo(entry_repo) == target_repo).then(|| digest.to_string())
    })
}

/// Strips whichever Docker Hub host/namespace prefix is present, so
/// "postgres", "library/postgres", "docker.io/library/postgres", and
/// "index.docker.io/library/postgres" all normalize to "postgres" —
/// and two-part Hub images like "valkey/valkey" normalize the same
/// way whether or not a "docker.io/" host prefix is present.
fn normalize_docker_repo(repo: &str) -> &str {
    for prefix in [
        "index.docker.io/library/",
        "index.docker.io/",
        "docker.io/library/",
        "docker.io/",
        "library/",
    ] {
        if let Some(stripped) = repo.strip_prefix(prefix) {
            return stripped;
        }
    }
    repo
}

/// "nginx:1.25" -> "nginx", "registry.local:5000/app:1.0" -> "registry.local:5000/app",
/// "nginx" -> "nginx" (no tag). Only strips a trailing `:tag` when the
/// text after the last colon doesn't itself contain a `/`, since a
/// colon before a `/` is a registry port, not a tag separator.
fn image_repo(image: &str) -> &str {
    match image.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') => repo,
        _ => image,
    }
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod repo_matching_tests {
    use super::*;

    #[test]
    fn matches_official_image_against_library_prefix() {
        let json = r#"["docker.io/library/postgres@sha256:aaaa"]"#;
        assert_eq!(
            extract_matching_digest(json, "postgres:16"),
            Some("sha256:aaaa".to_string())
        );
    }

    #[test]
    fn matches_two_part_hub_image_with_or_without_host_prefix() {
        let json = r#"["docker.io/valkey/valkey@sha256:bbbb"]"#;
        assert_eq!(
            extract_matching_digest(json, "valkey/valkey:7"),
            Some("sha256:bbbb".to_string())
        );
    }

    #[test]
    fn matches_fully_qualified_third_party_image_unchanged() {
        let json = r#"["ghcr.io/flaresolverr/flaresolverr@sha256:cccc"]"#;
        assert_eq!(
            extract_matching_digest(json, "ghcr.io/flaresolverr/flaresolverr:latest"),
            Some("sha256:cccc".to_string())
        );
    }
}

#[cfg(test)]
mod parse_image_ref_tests {
    use super::*;

    #[test]
    fn plain_tag_no_digest() {
        let p = parse_image_ref("postgres:16");
        assert_eq!(p.repo, "postgres");
        assert_eq!(p.tag, Some("16"));
        assert_eq!(p.digest, None);
    }

    #[test]
    fn immich_pinned_multi_dash_tag_with_digest() {
        // The exact reference reported: a tag containing dots and
        // dashes, *then* a digest pin — the naive "split on last
        // colon" approach grabs the hex digest as the "tag" here,
        // which is the bug this parser exists to avoid.
        let p = parse_image_ref(
            "ghcr.io/immich-app/postgres:14-vectorchord0.4.3-pgvectors0.2.0@sha256:bcf63357191b76a916ae5eb93464d65c07511da41e3bf7a8416db519b40b1c2",
        );
        assert_eq!(p.repo, "ghcr.io/immich-app/postgres");
        assert_eq!(p.tag, Some("14-vectorchord0.4.3-pgvectors0.2.0"));
        assert_eq!(
            p.digest,
            Some("sha256:bcf63357191b76a916ae5eb93464d65c07511da41e3bf7a8416db519b40b1c2")
        );
    }

    #[test]
    fn valkey_pinned_short_tag_with_digest() {
        let p = parse_image_ref(
            "docker.io/valkey/valkey:9@sha256:fb8d272e529ea567b9bf1302245796f21a2672b8368ca3fcb938ac334e613c8",
        );
        assert_eq!(p.repo, "docker.io/valkey/valkey");
        assert_eq!(p.tag, Some("9"));
        assert_eq!(
            p.digest,
            Some("sha256:fb8d272e529ea567b9bf1302245796f21a2672b8368ca3fcb938ac334e613c8")
        );
    }

    #[test]
    fn digest_only_no_tag() {
        let p = parse_image_ref("postgres@sha256:aaaa");
        assert_eq!(p.repo, "postgres");
        assert_eq!(p.tag, None);
        assert_eq!(p.digest, Some("sha256:aaaa"));
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawService {
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "Service", default)]
    service: String,
    #[serde(rename = "Image", default)]
    image: String,
    #[serde(rename = "State", default)]
    state: String,
    #[serde(rename = "Status", default)]
    status: String,
    #[serde(rename = "Health", default)]
    health: String,
    #[serde(rename = "Ports", default)]
    ports: String,
    #[serde(rename = "Publishers", default)]
    publishers: Vec<RawPublisher>,
}

#[derive(Debug, Default, Deserialize)]
struct RawPublisher {
    #[serde(rename = "URL", default)]
    url: String,
    #[serde(rename = "TargetPort", default)]
    target_port: u32,
    #[serde(rename = "PublishedPort", default)]
    published_port: u32,
    #[serde(rename = "Protocol", default)]
    protocol: String,
}

impl RawService {
    fn into_service(self) -> ComposeService {
        let ports = if !self.ports.is_empty() {
            self.ports
        } else {
            self.publishers
                .iter()
                .filter(|p| p.published_port != 0)
                .map(|p| {
                    format!(
                        "{}:{}->{}/{}",
                        p.url, p.published_port, p.target_port, p.protocol
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        ComposeService {
            name: self.name,
            service: self.service,
            image: self.image,
            state: self.state,
            status: self.status,
            health: if self.health.is_empty() {
                None
            } else {
                Some(self.health)
            },
            ports,
        }
    }
}
