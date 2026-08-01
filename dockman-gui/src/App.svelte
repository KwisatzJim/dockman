<script>
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  let hosts = [];
  let statuses = {}; // `${hostId}::${dirId}` -> StackStatus
  let selectedHostId = null;
  let selectedDirId = null;
  let busyKeys = new Set(); // stack keys currently running an action
  let refreshingAll = false;
  let statusMessage = "";
  let versionChecks = {}; // stack key -> { checking, results: {image: ImageVersionInfo} }

  let showAddHost = false;
  let sshAliases = [];
  let newHost = { ssh_alias: "", label: "" };
  let testResult = null;
  let testing = false;

  let showAddDir = false;
  let newDir = { path: "", label: "" };

  function key(hostId, dirId) {
    return `${hostId}::${dirId}`;
  }

  $: selectedHost = hosts.find((h) => h.id === selectedHostId) ?? null;
  $: selectedDir = selectedHost?.compose_dirs.find((d) => d.id === selectedDirId) ?? null;
  $: selectedStatus = selectedHostId && selectedDirId ? statuses[key(selectedHostId, selectedDirId)] : null;

  $: selectedVersions = selectedHostId && selectedDirId ? versionChecks[key(selectedHostId, selectedDirId)] : null;

  async function checkVersions(hostId, dirId) {
    const k = key(hostId, dirId);
    versionChecks = { ...versionChecks, [k]: { checking: true, results: versionChecks[k]?.results ?? {} } };
    statusMessage = "Checking registry for newer images...";
    try {
      const infos = await invoke("check_stack_versions", { hostId, dirId });
      const results = {};
      for (const info of infos) results[info.image] = info;
      const updates = infos.filter((i) => i.status === "update_available").length;
      statusMessage =
        updates === 0
          ? "All images up to date (or unknown)."
          : `${updates} image(s) have an update available.`;
      versionChecks = { ...versionChecks, [k]: { checking: false, results } };
    } catch (e) {
      statusMessage = `Version check failed: ${e}`;
      versionChecks = { ...versionChecks, [k]: { checking: false, results: versionChecks[k]?.results ?? {} } };
    }
  }

  function shortDigest(d) {
    return d ? d.replace("sha256:", "").slice(0, 12) : "?";
  }

  async function refreshHosts() {
    hosts = await invoke("list_hosts");
    if (!selectedHostId && hosts.length > 0) {
      selectedHostId = hosts[0].id;
    }
  }

  async function refreshEverything() {
    refreshingAll = true;
    statusMessage = "Checking every stack over ssh...";
    try {
      const results = await invoke("refresh_everything");
      const next = {};
      for (const s of results) next[key(s.host_id, s.dir_id)] = s;
      statuses = next;
      statusMessage = `Checked ${results.length} stack(s).`;
    } catch (e) {
      statusMessage = `Refresh failed: ${e}`;
    } finally {
      refreshingAll = false;
    }
  }

  async function refreshOne(hostId, dirId) {
    const k = key(hostId, dirId);
    const status = await invoke("refresh_stack", { hostId, dirId });
    statuses = { ...statuses, [k]: status };
  }

  async function openAddHost() {
    testResult = null;
    newHost = { ssh_alias: "", label: "" };
    try {
      sshAliases = await invoke("list_ssh_aliases");
    } catch (e) {
      sshAliases = [];
    }
    showAddHost = true;
  }

  async function testConnection() {
    if (!newHost.ssh_alias) return;
    testing = true;
    testResult = null;
    try {
      const [ok, output] = await invoke("check_host", { alias: newHost.ssh_alias });
      testResult = ok ? "Reachable." : `Failed: ${output}`;
    } catch (e) {
      testResult = `Failed: ${e}`;
    } finally {
      testing = false;
    }
  }

  async function addHost() {
    if (!newHost.ssh_alias) return;
    await invoke("add_host", {
      sshAlias: newHost.ssh_alias,
      label: newHost.label || newHost.ssh_alias,
    });
    showAddHost = false;
    await refreshHosts();
    selectedHostId = newHost.ssh_alias;
  }

  async function removeHost(id) {
    await invoke("remove_host", { id });
    if (selectedHostId === id) {
      selectedHostId = null;
      selectedDirId = null;
    }
    await refreshHosts();
  }

  function openAddDir() {
    newDir = { path: "", label: "" };
    showAddDir = true;
  }

  async function addDir() {
    if (!selectedHostId || !newDir.path) return;
    await invoke("add_compose_dir", {
      hostId: selectedHostId,
      path: newDir.path,
      label: newDir.label,
    });
    showAddDir = false;
    await refreshHosts();
    await refreshOne(selectedHostId, newDir.path);
    selectedDirId = newDir.path;
  }

  async function removeDir(hostId, dirId) {
    await invoke("remove_compose_dir", { hostId, dirId });
    if (selectedDirId === dirId) selectedDirId = null;
    await refreshHosts();
  }

  function selectStack(hostId, dirId) {
    selectedHostId = hostId;
    selectedDirId = dirId;
    if (!statuses[key(hostId, dirId)]) refreshOne(hostId, dirId);
  }

  async function runOnStack(hostId, dirId, action) {
    const k = key(hostId, dirId);
    busyKeys = new Set(busyKeys).add(k);
    statusMessage = `${action} running on ${dirId}...`;
    try {
      const outcome = await invoke("run_stack_action", { hostId, dirId, action });
      statusMessage = outcome.ok
        ? `${action} succeeded on ${dirId}.`
        : `${action} failed on ${dirId}: ${outcome.output}`;
    } catch (e) {
      statusMessage = `${action} failed: ${e}`;
    } finally {
      busyKeys.delete(k);
      busyKeys = new Set(busyKeys);
      await refreshOne(hostId, dirId);
    }
  }

  async function runOnHost(hostId, action) {
    statusMessage = `${action} running on every stack for this host...`;
    try {
      const outcomes = await invoke("run_action_on_host", { hostId, action });
      const failed = outcomes.filter((o) => !o.ok);
      statusMessage =
        failed.length === 0
          ? `${action} succeeded on all ${outcomes.length} stack(s).`
          : `${action}: ${failed.length}/${outcomes.length} stack(s) failed.`;
    } catch (e) {
      statusMessage = `${action} failed: ${e}`;
    } finally {
      await refreshEverything();
    }
  }

  async function runEverywhere(action) {
    statusMessage = `${action} running across every host...`;
    try {
      const outcomes = await invoke("run_action_everywhere", { action });
      const failed = outcomes.filter((o) => !o.ok);
      statusMessage =
        failed.length === 0
          ? `${action} succeeded on all ${outcomes.length} stack(s).`
          : `${action}: ${failed.length}/${outcomes.length} stack(s) failed.`;
    } catch (e) {
      statusMessage = `${action} failed: ${e}`;
    } finally {
      await refreshEverything();
    }
  }

  function overallState(status) {
    if (!status) return "unknown";
    if (status.error) return "error";
    if (status.services.length === 0) return "empty";
    if (status.services.every((s) => s.state === "running")) return "running";
    if (status.services.some((s) => s.state === "running")) return "partial";
    return "stopped";
  }

  onMount(async () => {
    await refreshHosts();
    await refreshEverything();
  });
</script>

<main>
  <aside>
    <div class="sidebar-header">
      <h2>Hosts</h2>
      <button on:click={openAddHost}>+ Add</button>
    </div>

    <div class="global-actions">
      <button on:click={refreshEverything} disabled={refreshingAll}>
        {refreshingAll ? "Refreshing..." : "Refresh all"}
      </button>
      <button class="primary" on:click={() => runEverywhere("update")}>
        Update all
      </button>
    </div>

    <ul class="host-list">
      {#each hosts as host}
        <li class="host">
          <div class="host-row">
            <span class="host-label" on:click={() => (selectedHostId = host.id)}>
              {host.label}
            </span>
            <button
              class="icon"
              title="Update all stacks on this host"
              on:click={() => runOnHost(host.id, "update")}>&#8635;</button
            >
            <button class="icon remove" title="Remove host" on:click={() => removeHost(host.id)}
              >x</button
            >
          </div>
          <ul class="dir-list">
            {#each host.compose_dirs as dir}
              {@const status = statuses[key(host.id, dir.id)]}
              <li
                class:active={host.id === selectedHostId && dir.id === selectedDirId}
                on:click={() => selectStack(host.id, dir.id)}
              >
                <span class="dot {overallState(status)}"></span>
                <span class="dir-label">{dir.label}</span>
              </li>
            {/each}
            {#if host.id === selectedHostId}
              <li class="add-dir" on:click={openAddDir}>+ Add stack directory</li>
            {/if}
          </ul>
        </li>
      {/each}
    </ul>
  </aside>

  <section class="content">
    {#if statusMessage}
      <p class="status">{statusMessage}</p>
    {/if}

    {#if !selectedDir}
      <p class="empty">
        Select a stack on the left, or add a host / compose directory to get started.
      </p>
    {:else}
      <div class="content-header">
        <div>
          <h1>{selectedDir.label}</h1>
          <p class="path mono">{selectedHost.label}:{selectedDir.path}</p>
        </div>
        <div class="stack-actions">
          <button on:click={() => refreshOne(selectedHostId, selectedDirId)}>Refresh</button>
          <button
            disabled={selectedVersions?.checking}
            on:click={() => checkVersions(selectedHostId, selectedDirId)}
            >{selectedVersions?.checking ? "Checking..." : "Check for updates"}</button
          >
          <button on:click={() => runOnStack(selectedHostId, selectedDirId, "start")}
            >Start</button
          >
          <button on:click={() => runOnStack(selectedHostId, selectedDirId, "restart")}
            >Restart</button
          >
          <button on:click={() => runOnStack(selectedHostId, selectedDirId, "stop")}
            >Stop</button
          >
          <button
            class="primary"
            disabled={busyKeys.has(key(selectedHostId, selectedDirId))}
            on:click={() => runOnStack(selectedHostId, selectedDirId, "update")}
            >{busyKeys.has(key(selectedHostId, selectedDirId)) ? "Updating..." : "Update"}</button
          >
          <button class="danger" on:click={() => removeDir(selectedHostId, selectedDirId)}
            >Remove from list</button
          >
        </div>
      </div>

      {#if selectedStatus?.error}
        <p class="error-box">{selectedStatus.error}</p>
      {:else if selectedStatus}
        <table>
          <thead>
            <tr>
              <th>Service</th>
              <th>Image</th>
              <th>State</th>
              <th>Status</th>
              <th>Version</th>
              <th>Ports</th>
            </tr>
          </thead>
          <tbody>
            {#each selectedStatus.services as s}
              <tr>
                <td>{s.service}</td>
                <td class="mono">{s.image}</td>
                <td><span class="badge {s.state}">{s.state}</span></td>
                <td>{s.status}{s.health ? ` (${s.health})` : ""}</td>
                <td>
                  {#if selectedVersions?.results?.[s.image]}
                    {@const v = selectedVersions.results[s.image]}
                    <span
                      class="badge version-{v.status}"
                      title="running {shortDigest(v.running_digest)} / latest {shortDigest(v.latest_digest)}"
                    >
                      {v.status === "up_to_date"
                        ? "up to date"
                        : v.status === "update_available"
                          ? "update available"
                          : "unknown"}
                    </span>
                  {:else}
                    <span class="mono">—</span>
                  {/if}
                </td>
                <td class="mono">{s.ports}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if selectedStatus.services.length === 0}
          <p class="empty">No services reported — is the compose file in this directory?</p>
        {/if}
      {/if}
    {/if}
  </section>
</main>

{#if showAddHost}
  <div class="modal-backdrop" on:click={() => (showAddHost = false)}>
    <div class="modal" on:click|stopPropagation>
      <h2>Add host</h2>
      <p class="hint">
        Picked from Host entries in ~/.ssh/config. Whatever auth that alias
        already uses (key, ProxyJump, etc.) is what dockman will use too.
      </p>
      <label>
        SSH alias
        {#if sshAliases.length > 0}
          <select bind:value={newHost.ssh_alias}>
            <option value="" disabled selected>Select an alias...</option>
            {#each sshAliases as alias}
              <option value={alias}>{alias}</option>
            {/each}
          </select>
        {:else}
          <input bind:value={newHost.ssh_alias} placeholder="e.g. cachyos-box" />
        {/if}
      </label>
      <label>
        Label (optional)
        <input bind:value={newHost.label} placeholder={newHost.ssh_alias || "display name"} />
      </label>
      <div class="test-row">
        <button on:click={testConnection} disabled={testing || !newHost.ssh_alias}>
          {testing ? "Testing..." : "Test connection"}
        </button>
        {#if testResult}<span class="test-result">{testResult}</span>{/if}
      </div>
      <div class="modal-actions">
        <button on:click={() => (showAddHost = false)}>Cancel</button>
        <button class="primary" on:click={addHost} disabled={!newHost.ssh_alias}>Add</button>
      </div>
    </div>
  </div>
{/if}

{#if showAddDir}
  <div class="modal-backdrop" on:click={() => (showAddDir = false)}>
    <div class="modal" on:click|stopPropagation>
      <h2>Add stack directory</h2>
      <p class="hint">
        Absolute path on {selectedHost?.label} containing docker-compose.yml.
      </p>
      <label>
        Path
        <input bind:value={newDir.path} placeholder="/home/jim/docker/nextcloud" />
      </label>
      <label>
        Label (optional)
        <input bind:value={newDir.label} placeholder="nextcloud" />
      </label>
      <div class="modal-actions">
        <button on:click={() => (showAddDir = false)}>Cancel</button>
        <button class="primary" on:click={addDir} disabled={!newDir.path}>Add</button>
      </div>
    </div>
  </div>
{/if}

<style>
  :global(body) {
    margin: 0;
    font-family: -apple-system, "Segoe UI", sans-serif;
    background: #14161a;
    color: #e7e9ec;
  }

  main {
    display: flex;
    height: 100vh;
  }

  aside {
    width: 260px;
    background: #1b1e24;
    border-right: 1px solid #2a2e36;
    display: flex;
    flex-direction: column;
    padding: 12px;
    overflow-y: auto;
  }

  .sidebar-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .sidebar-header h2 {
    font-size: 14px;
    text-transform: uppercase;
    color: #8b92a1;
    margin: 0;
  }

  .global-actions {
    display: flex;
    gap: 6px;
    margin: 10px 0;
  }

  .global-actions button {
    flex: 1;
  }

  .host-list {
    list-style: none;
    padding: 0;
    margin: 4px 0;
  }

  .host {
    margin-bottom: 6px;
  }

  .host-row {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 8px;
    border-radius: 6px;
  }

  .host-row:hover {
    background: #23272f;
  }

  .host-label {
    flex: 1;
    font-weight: 600;
    cursor: pointer;
  }

  .dir-list {
    list-style: none;
    padding-left: 14px;
    margin: 2px 0;
  }

  .dir-list li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 13px;
  }

  .dir-list li:hover {
    background: #23272f;
  }

  .dir-list li.active {
    background: #2f6feb33;
    color: #6fa8ff;
  }

  .dir-list li.add-dir {
    color: #8b92a1;
    font-style: italic;
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #4b5160;
    flex-shrink: 0;
  }

  .dot.running {
    background: #5fd583;
  }

  .dot.stopped {
    background: #4b5160;
  }

  .dot.partial {
    background: #e0b95f;
  }

  .dot.error {
    background: #e07070;
  }

  .icon {
    background: none;
    border: none;
    color: #8b92a1;
    cursor: pointer;
    padding: 2px 6px;
    font-size: 13px;
  }

  .icon:hover {
    color: #e7e9ec;
  }

  .content {
    flex: 1;
    padding: 20px 28px;
    overflow-y: auto;
  }

  .content-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
  }

  .content-header h1 {
    margin: 0 0 4px 0;
  }

  .path {
    margin: 0;
  }

  .stack-actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .status {
    color: #8b92a1;
    font-size: 13px;
  }

  .error-box {
    background: #3d1c1c;
    color: #e07070;
    padding: 12px;
    border-radius: 8px;
    white-space: pre-wrap;
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 16px;
  }

  th,
  td {
    text-align: left;
    padding: 8px 10px;
    border-bottom: 1px solid #2a2e36;
    font-size: 13px;
  }

  .mono {
    font-family: ui-monospace, monospace;
    color: #8b92a1;
    font-size: 12px;
  }

  .badge {
    padding: 2px 8px;
    border-radius: 10px;
    font-size: 11px;
    background: #2a2e36;
  }

  .badge.running {
    background: #1c3d2a;
    color: #5fd583;
  }

  .badge.exited {
    background: #3d1c1c;
    color: #e07070;
  }

  .badge.version-up_to_date {
    background: #1c3d2a;
    color: #5fd583;
  }

  .badge.version-update_available {
    background: #3d341c;
    color: #e0b95f;
  }

  .badge.version-unknown {
    background: #2a2e36;
    color: #8b92a1;
  }

  button {
    background: #23272f;
    border: 1px solid #2a2e36;
    color: #e7e9ec;
    padding: 6px 10px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 12px;
  }

  button:hover {
    background: #2a2e36;
  }

  button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  button.danger {
    color: #e07070;
  }

  button.primary {
    background: #2f6feb;
    border-color: #2f6feb;
  }

  .empty {
    color: #8b92a1;
    margin-top: 24px;
  }

  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: #00000080;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .modal {
    background: #1b1e24;
    border: 1px solid #2a2e36;
    border-radius: 10px;
    padding: 20px;
    width: 360px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .modal h2 {
    margin: 0;
  }

  .modal label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: #8b92a1;
  }

  .modal input,
  .modal select {
    background: #14161a;
    border: 1px solid #2a2e36;
    color: #e7e9ec;
    padding: 6px 8px;
    border-radius: 6px;
    font-size: 13px;
  }

  .test-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .test-result {
    font-size: 12px;
    color: #8b92a1;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }

  .hint {
    font-size: 12px;
    color: #8b92a1;
    margin: 0;
  }
</style>
