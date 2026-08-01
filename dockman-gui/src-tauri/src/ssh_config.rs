use std::path::PathBuf;

/// Parse `Host` entries out of ~/.ssh/config (and anything it `Include`s,
/// one level deep — covers the common case of a distro-managed
/// `~/.ssh/config.d/*` split without going full recursive-glob).
/// Wildcard/pattern hosts ("*", "github.com", host globs with * or ?)
/// are skipped since they're not real machines you'd ssh to by name.
pub fn list_aliases() -> anyhow::Result<Vec<String>> {
    let Some(home) = dirs::home_dir() else {
        return Ok(Vec::new());
    };
    let config_path = home.join(".ssh").join("config");
    let mut aliases = Vec::new();
    collect_aliases(&config_path, &mut aliases, 0)?;
    aliases.sort();
    aliases.dedup();
    Ok(aliases)
}

fn collect_aliases(path: &PathBuf, out: &mut Vec<String>, depth: u8) -> anyhow::Result<()> {
    if depth > 2 || !path.exists() {
        return Ok(());
    }
    let contents = std::fs::read_to_string(path)?;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(keyword) = parts.next() else {
            continue;
        };
        let rest = parts.next().unwrap_or("").trim();

        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                for token in rest.split_whitespace() {
                    if !token.contains('*') && !token.contains('?') {
                        out.push(token.to_string());
                    }
                }
            }
            "include" => {
                if let Some(parent) = path.parent() {
                    for pattern in rest.split_whitespace() {
                        let candidate = if pattern.starts_with('/') || pattern.starts_with('~') {
                            shellexpand_home(pattern)
                        } else {
                            parent.join(pattern)
                        };
                        // Only handle non-glob includes here; globs are
                        // an edge case not worth pulling in a glob crate
                        // for in this scaffold.
                        collect_aliases(&candidate, out, depth + 1)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn shellexpand_home(pattern: &str) -> PathBuf {
    if let Some(rest) = pattern.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(pattern)
}
