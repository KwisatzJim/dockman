mod ssh_config;
mod ssh_runner;
mod state;

use dockman_core::{ActionOutcome, ComposeDir, HostConfig, ImageVersionInfo, StackAction, StackStatus};
use futures_util::future::join_all;
use state::HostStore;
use tauri::State;

struct AppContext {
    hosts: HostStore,
}

#[tauri::command]
async fn list_hosts(ctx: State<'_, AppContext>) -> Result<Vec<HostConfig>, String> {
    Ok(ctx.hosts.list())
}

#[tauri::command]
async fn list_ssh_aliases() -> Result<Vec<String>, String> {
    ssh_config::list_aliases().map_err(|e| e.to_string())
}

#[tauri::command]
async fn check_host(alias: String) -> Result<(bool, String), String> {
    ssh_runner::check_host(&alias).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_host(ctx: State<'_, AppContext>, ssh_alias: String, label: String) -> Result<(), String> {
    ctx.hosts
        .upsert(HostConfig {
            id: ssh_alias.clone(),
            ssh_alias,
            label,
            compose_dirs: Vec::new(),
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_host(ctx: State<'_, AppContext>, id: String) -> Result<(), String> {
    ctx.hosts.remove(&id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_compose_dir(
    ctx: State<'_, AppContext>,
    host_id: String,
    path: String,
    label: String,
) -> Result<(), String> {
    let mut host = ctx.hosts.get(&host_id).ok_or("unknown host")?;
    let label = if label.is_empty() {
        path.rsplit('/').next().unwrap_or(&path).to_string()
    } else {
        label
    };
    host.compose_dirs.push(ComposeDir {
        id: path.clone(),
        path,
        label,
    });
    ctx.hosts.upsert(host).map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_compose_dir(
    ctx: State<'_, AppContext>,
    host_id: String,
    dir_id: String,
) -> Result<(), String> {
    let mut host = ctx.hosts.get(&host_id).ok_or("unknown host")?;
    host.compose_dirs.retain(|d| d.id != dir_id);
    ctx.hosts.upsert(host).map_err(|e| e.to_string())
}

#[tauri::command]
async fn refresh_stack(
    ctx: State<'_, AppContext>,
    host_id: String,
    dir_id: String,
) -> Result<StackStatus, String> {
    let host = ctx.hosts.get(&host_id).ok_or("unknown host")?;
    let dir = host
        .compose_dirs
        .iter()
        .find(|d| d.id == dir_id)
        .ok_or("unknown compose dir")?
        .clone();
    Ok(ssh_runner::fetch_stack_status(&host, &dir).await)
}

#[tauri::command]
async fn refresh_everything(ctx: State<'_, AppContext>) -> Result<Vec<StackStatus>, String> {
    let hosts = ctx.hosts.list();
    let futures = hosts.iter().flat_map(|host| {
        host.compose_dirs
            .iter()
            .map(move |dir| ssh_runner::fetch_stack_status(host, dir))
    });
    Ok(join_all(futures).await)
}

/// "Check for updates" for one stack: re-lists its services, then
/// checks each *unique* image's digest against the registry in
/// parallel (a stack with 4 services on 2 images only does 2 checks).
#[tauri::command]
async fn check_stack_versions(
    ctx: State<'_, AppContext>,
    host_id: String,
    dir_id: String,
) -> Result<Vec<ImageVersionInfo>, String> {
    let host = ctx.hosts.get(&host_id).ok_or("unknown host")?;
    let dir = host
        .compose_dirs
        .iter()
        .find(|d| d.id == dir_id)
        .ok_or("unknown compose dir")?
        .clone();

    let status = ssh_runner::fetch_stack_status(&host, &dir).await;
    if let Some(err) = status.error {
        return Err(err);
    }

    let mut images: Vec<String> = status.services.into_iter().map(|s| s.image).collect();
    images.sort();
    images.dedup();

    let futures = images
        .iter()
        .map(|image| ssh_runner::check_image_version(&host, image));
    Ok(join_all(futures).await)
}

#[tauri::command]
async fn run_stack_action(
    ctx: State<'_, AppContext>,
    host_id: String,
    dir_id: String,
    action: StackAction,
) -> Result<ActionOutcome, String> {
    let host = ctx.hosts.get(&host_id).ok_or("unknown host")?;
    let dir = host
        .compose_dirs
        .iter()
        .find(|d| d.id == dir_id)
        .ok_or("unknown compose dir")?
        .clone();
    Ok(ssh_runner::run_action(&host, &dir, action).await)
}

#[tauri::command]
async fn run_action_everywhere(
    ctx: State<'_, AppContext>,
    action: StackAction,
) -> Result<Vec<ActionOutcome>, String> {
    let hosts = ctx.hosts.list();
    let futures = hosts.iter().flat_map(|host| {
        host.compose_dirs
            .iter()
            .map(move |dir| ssh_runner::run_action(host, dir, action))
    });
    Ok(join_all(futures).await)
}

#[tauri::command]
async fn run_action_on_host(
    ctx: State<'_, AppContext>,
    host_id: String,
    action: StackAction,
) -> Result<Vec<ActionOutcome>, String> {
    let host = ctx.hosts.get(&host_id).ok_or("unknown host")?;
    let futures = host
        .compose_dirs
        .iter()
        .map(|dir| ssh_runner::run_action(&host, dir, action));
    Ok(join_all(futures).await)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();

    let hosts = HostStore::load().expect("could not load hosts.json");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppContext { hosts })
        .invoke_handler(tauri::generate_handler![
            list_hosts,
            list_ssh_aliases,
            check_host,
            add_host,
            remove_host,
            add_compose_dir,
            remove_compose_dir,
            refresh_stack,
            refresh_everything,
            check_stack_versions,
            run_stack_action,
            run_action_everywhere,
            run_action_on_host,
        ])
        .run(tauri::generate_context!())
        .expect("error while running dockman");
}
