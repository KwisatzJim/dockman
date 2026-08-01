use anyhow::{Context, Result};
use dockman_core::HostConfig;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct HostStore {
    inner: Mutex<Vec<HostConfig>>,
    path: PathBuf,
}

impl HostStore {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        let hosts: Vec<HostConfig> = if path.exists() {
            let raw = std::fs::read_to_string(&path).context("reading hosts.json")?;
            serde_json::from_str(&raw).context("parsing hosts.json")?
        } else {
            Vec::new()
        };
        Ok(Self {
            inner: Mutex::new(hosts),
            path,
        })
    }

    pub fn list(&self) -> Vec<HostConfig> {
        self.inner.lock().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Option<HostConfig> {
        self.inner.lock().unwrap().iter().find(|h| h.id == id).cloned()
    }

    pub fn upsert(&self, host: HostConfig) -> Result<()> {
        let mut hosts = self.inner.lock().unwrap();
        if let Some(existing) = hosts.iter_mut().find(|h| h.id == host.id) {
            *existing = host;
        } else {
            hosts.push(host);
        }
        self.persist(&hosts)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut hosts = self.inner.lock().unwrap();
        hosts.retain(|h| h.id != id);
        self.persist(&hosts)
    }

    /// Replace the whole set at once — used when editing a host's
    /// compose_dirs list from the UI, simpler than a bunch of granular
    /// add/remove-dir commands. Not called yet by any command below;
    /// kept for when the UI grows a bulk-edit view.
    #[allow(dead_code)]
    pub fn replace_all(&self, hosts: Vec<HostConfig>) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        *guard = hosts;
        self.persist(&guard)
    }

    fn persist(&self, hosts: &[HostConfig]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(hosts)?;
        std::fs::write(&self.path, raw).context("writing hosts.json")
    }
}

fn config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("could not determine config dir")?
        .join("dockman");
    Ok(dir.join("hosts.json"))
}
