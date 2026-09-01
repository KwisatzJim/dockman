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
  let scanningLan = false;
  let discoveredHosts = [];
  let showLanScan = false;
  let scanningHostId = null;

  let showAddDir = false;
  let newDir = { path: "", label: "" };

  const MAINTENANCE_ACTIONS = [
    {
      value: "prune_images",
      label: "Prune unused images",
      desc: "Removes every image not currently backing a container — including images for stopped stacks. Re-running a stopped stack afterward means re-pulling.",
      dataRisk: false,
    },
    {
      value: "prune_containers",
      label: "Prune stopped containers",
      desc: "Removes stopped containers. Low risk — anything still running is untouched, and compose recreates containers from the compose file anyway.",
      dataRisk: false,
    },
    {
      value: "prune_volumes",
      label: "Prune unused volumes",
      desc: "Removes volumes not attached to any container right now. Real data-loss risk if a stack is currently stopped — double check nothing you care about is only stopped, not removed, first.",
      dataRisk: true,
    },
    {
      value: "prune_build_cache",
      label: "Prune build cache",
      desc: "Removes the BuildKit build cache. Safe — only affects build speed next time you build an image.",
      dataRisk: false,
    },
    {
      value: "prune_system",
      label: "Prune system",
      desc: "Combines image + container + network pruning in one call. Does not touch volumes.",
      dataRisk: false,
    },
  ];

  let showMaintenance = false;
  let maintenanceHostId = null;
  let confirmPending = {}; // action value -> true while awaiting a second click
  let maintenanceBusy = null; // action value currently running, or null
  let maintenanceResults = {}; // hostId -> { action, ok, output }

  function openMaintenance(hostId) {
    maintenanceHostId = hostId;
    confirmPending = {};
    showMaintenance = true;
  }

  function requestMaintenance(action) {
    if (!confirmPending[action]) {
      confirmPending = { ...confirmPending, [action]: true };
      setTimeout(() => {
        // auto-reset the confirmation if they don't follow through
        confirmPending = { ...confirmPending, [action]: false };
      }, 4000);
      return;
    }
    confirmPending = { ...confirmPending, [action]: false };
    runMaintenance(action);
  }

  async function runMaintenance(action) {
    const hostId = maintenanceHostId;
    maintenanceBusy = action;
    statusMessage = `Running ${action} on ${hostId}...`;
    try {
      const outcome = await invoke("run_maintenance", { hostId, action });
      maintenanceResults = { ...maintenanceResults, [hostId]: outcome };
      statusMessage = outcome.ok
        ? `${action} finished on ${hostId}.`
        : `${action} failed on ${hostId}: ${outcome.output}`;
    } catch (e) {
      statusMessage = `${action} failed: ${e}`;
    } finally {
      maintenanceBusy = null;
    }
  }

  function key(hostId, dirId) {
    return `${hostId}::${dirId}`;
  }

  $: selectedHost = hosts.find((h) => h.id === selectedHostId) ?? null;
  $: selectedDir = selectedHost?.compose_dirs.find((d) => d.id === selectedDirId) ?? null;
  $: selectedStatus = selectedHostId && selectedDirId ? statuses[key(selectedHostId, selectedDirId)] : null;

  $: selectedVersions = selectedHostId && selectedDirId ? versionChecks[key(selectedHostId, selectedDirId)] : null;
  $: allContainers = hosts.flatMap((host) =>
    host.compose_dirs.flatMap((dir) =>
      (statuses[key(host.id, dir.id)]?.services ?? []).map((service) => ({ host, dir, service }))
    )
  );
  $: runningCount = allContainers.filter((item) => item.service.state === "running").length;
  $: updateCount = allContainers.filter((item) => versionChecks[key(item.host.id, item.dir.id)]?.results?.[item.service.image]?.status === "update_available").length;

  async function checkAllVersions() {
    const stacks = hosts.flatMap((host) => host.compose_dirs.map((dir) => [host.id, dir.id]));
    statusMessage = `Checking ${stacks.length} stack(s) for updates...`;
    await Promise.all(stacks.map(([hostId, dirId]) => checkVersions(hostId, dirId)));
    statusMessage = "Finished checking all container images.";
  }

  async function scanHost(hostId) {
    scanningHostId = hostId;
    statusMessage = "Reading Docker Compose labels on this PC...";
    try {
      const found = await invoke("scan_host_containers", { hostId });
      await refreshHosts();
      await refreshEverything();
      statusMessage = `Found ${found.length} Compose stack(s). New stacks were added automatically.`;
    } catch (e) {
      statusMessage = `Container scan failed: ${e}`;
    } finally {
      scanningHostId = null;
    }
  }

  async function scanLan() {
    showLanScan = true;
    scanningLan = true;
    discoveredHosts = [];
    try {
      discoveredHosts = await invoke("scan_lan");
    } catch (e) {
      statusMessage = `LAN scan failed: ${e}`;
    } finally {
      scanningLan = false;
    }
  }

  async function addDiscovered(host) {
    await invoke("add_discovered_host", { host });
    discoveredHosts = discoveredHosts.filter((item) => item.ssh_alias !== host.ssh_alias);
    await refreshHosts();
    await refreshEverything();
    statusMessage = `Added ${host.label} with ${host.compose_dirs.length} stack(s).`;
  }

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

  function showDashboard() {
    selectedDirId = null;
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
      <button class="brand" on:click={showDashboard} title="Open dashboard">
        <span class="brand-mark">D</span><span>Dockman</span>
      </button>
      <button on:click={openAddHost}>+ Add</button>
    </div>

    <button class="nav-dashboard" class:active={!selectedDirId} on:click={showDashboard}>Overview</button>

    <div class="global-actions">
      <button on:click={refreshEverything} disabled={refreshingAll}>
        {refreshingAll ? "Refreshing..." : "Refresh all"}
      </button>
      <button class="primary" on:click={() => runEverywhere("update")}>
        Update all
      </button>
    </div>
    <button class="scan-lan" on:click={scanLan} disabled={scanningLan}>
      {scanningLan ? "Scanning network..." : "Scan LAN for Docker PCs"}
    </button>

    <div class="section-label">Managed PCs</div>

    <ul class="host-list">
      {#each hosts as host}
        <li class="host">
          <div class="host-row">
            <span class="host-label" on:click={() => (selectedHostId = host.id)}>
              {host.label}
            </span>
            <button class="icon" title="Scan this PC for Compose stacks" disabled={scanningHostId === host.id} on:click={() => scanHost(host.id)}>&#128269;</button>
            <button
              class="icon"
              title="Update all stacks on this host"
              on:click={() => runOnHost(host.id, "update")}>&#8635;</button
            >
            <button class="icon" title="Docker maintenance" on:click={() => openMaintenance(host.id)}
              >&#129529;</button
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
      <div class="dashboard-header">
        <div>
          <p class="eyebrow">Fleet overview</p>
          <h1>Docker at a glance</h1>
          <p class="subtitle">Manage every discovered container from one place.</p>
        </div>
        <div class="dashboard-actions">
          <button on:click={refreshEverything} disabled={refreshingAll}>{refreshingAll ? "Refreshing..." : "Refresh status"}</button>
          <button on:click={checkAllVersions}>Check all for updates</button>
          <button on:click={() => runEverywhere("restart")}>Restart all</button>
          <button class="primary" on:click={() => runEverywhere("update")}>Update all</button>
        </div>
      </div>
      <div class="summary-grid">
        <div class="summary-card"><span>Managed PCs</span><strong>{hosts.length}</strong></div>
        <div class="summary-card"><span>Containers</span><strong>{allContainers.length}</strong></div>
        <div class="summary-card healthy"><span>Running</span><strong>{runningCount}</strong></div>
        <div class="summary-card updates"><span>Updates found</span><strong>{updateCount}</strong></div>
      </div>
      <div class="panel">
        <div class="panel-title"><h2>Containers</h2><span>{allContainers.length} total</span></div>
        {#if allContainers.length}
          <table class="container-table">
            <thead><tr><th>Container</th><th>PC / stack</th><th>Image</th><th>Status</th><th>Update</th><th></th></tr></thead>
            <tbody>
              {#each allContainers as item}
                {@const v = versionChecks[key(item.host.id, item.dir.id)]?.results?.[item.service.image]}
                <tr>
                  <td><button class="link-button" on:click={() => selectStack(item.host.id, item.dir.id)}>{item.service.name || item.service.service}</button></td>
                  <td><div>{item.host.label}</div><span class="mono">{item.dir.label}</span></td>
                  <td class="mono image-cell">{item.service.image}</td>
                  <td><span class="badge {item.service.state}">{item.service.state}</span></td>
                  <td>{#if v}<span class="badge version-{v.status}">{v.status === "update_available" ? "available" : v.status === "up_to_date" ? "current" : v.status === "pinned_by_compose" ? "Pinned by Compose" : "unknown"}</span>{:else}<span class="muted">not checked</span>{/if}</td>
                  <td class="row-actions"><button on:click={() => runOnStack(item.host.id, item.dir.id, "restart")}>Restart</button><button class="primary" on:click={() => runOnStack(item.host.id, item.dir.id, "update")}>Update</button></td>
                </tr>
              {/each}
            </tbody>
          </table>
        {:else}
          <div class="empty-state"><div class="empty-icon">⬡</div><h2>No containers yet</h2><p>Add a PC, then use its scan button to discover running Compose stacks.</p></div>
        {/if}
      </div>
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
                          : v.status === "pinned_by_compose"
                            ? "Pinned by Compose"
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

{#if showLanScan}
  <div class="modal-backdrop" on:click={() => (showLanScan = false)}>
    <div class="modal scan-modal" on:click|stopPropagation>
      <h2>Docker PCs on your network</h2>
      <p class="hint">Dockman checks your SSH aliases and nearby PCs already visible to this computer. A PC appears only when SSH key access and Docker both work.</p>
      {#if scanningLan}
        <div class="scanning"><span class="spinner"></span>Scanning reachable PCs…</div>
      {:else if discoveredHosts.length === 0}
        <div class="empty-state compact"><p>No new Docker PCs were reachable. Make sure SSH key access is configured first.</p></div>
      {:else}
        <ul class="discovery-list">
          {#each discoveredHosts as host}
            <li><div><strong>{host.label}</strong><span>{host.compose_dirs.length} Compose stack(s) found</span></div><button class="primary" on:click={() => addDiscovered(host)}>Add PC</button></li>
          {/each}
        </ul>
      {/if}
      <div class="modal-actions"><button on:click={() => (showLanScan = false)}>Close</button></div>
    </div>
  </div>
{/if}

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

{#if showMaintenance}
  <div class="modal-backdrop" on:click={() => (showMaintenance = false)}>
    <div class="modal maintenance-modal" on:click|stopPropagation>
      <h2>Docker maintenance — {hosts.find((h) => h.id === maintenanceHostId)?.label}</h2>
      <p class="hint">
        These act on the whole Docker daemon on this host, not any one stack. Click once to
        arm, click again within a few seconds to actually run it.
      </p>
      <ul class="maintenance-list">
        {#each MAINTENANCE_ACTIONS as a}
          <li>
            <div class="maintenance-row">
              <div>
                <div class="maintenance-label">
                  {a.label}
                  {#if a.dataRisk}<span class="risk-badge">data risk</span>{/if}
                </div>
                <div class="maintenance-desc">{a.desc}</div>
              </div>
              <button
                class={confirmPending[a.value] ? "danger" : ""}
                disabled={maintenanceBusy === a.value}
                on:click={() => requestMaintenance(a.value)}
              >
                {maintenanceBusy === a.value
                  ? "Running..."
                  : confirmPending[a.value]
                    ? "Click to confirm"
                    : "Run"}
              </button>
            </div>
          </li>
        {/each}
      </ul>
      {#if maintenanceResults[maintenanceHostId]}
        {@const r = maintenanceResults[maintenanceHostId]}
        <p class="hint">Last result ({r.action}, {r.ok ? "ok" : "failed"}):</p>
        <pre class="maintenance-output">{r.output}</pre>
      {/if}
      <div class="modal-actions">
        <button on:click={() => (showMaintenance = false)}>Close</button>
      </div>
    </div>
  </div>
{/if}

<style>
  :global(body) {
    margin: 0;
    font-family: Inter, ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    background: #090d16;
    color: #eef3fb;
  }

  main {
    display: flex;
    height: 100vh;
  }

  aside {
    width: 278px;
    box-sizing: border-box;
    background: linear-gradient(180deg, #111827 0%, #0c1220 100%);
    border-right: 1px solid #263247;
    display: flex;
    flex-direction: column;
    padding: 18px 14px;
    overflow-y: auto;
  }

  .sidebar-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
    background: transparent;
    border: 0;
    padding: 0;
    font-size: 18px;
    font-weight: 750;
  }

  .brand-mark {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 9px;
    color: white;
    background: linear-gradient(135deg, #28b6f6, #6366f1);
    box-shadow: 0 8px 24px #2563eb55;
  }

  .nav-dashboard, .scan-lan {
    width: 100%;
    margin-top: 16px;
    text-align: left;
    padding: 9px 11px;
  }

  .nav-dashboard.active { background: #2563eb22; border-color: #3b82f680; color: #8ec5ff; }
  .scan-lan { margin-top: 0; text-align: center; color: #8ec5ff; }
  .section-label { margin: 20px 8px 8px; color: #68758b; text-transform: uppercase; letter-spacing: .11em; font-size: 10px; font-weight: 700; }

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
    padding: 28px 34px;
    overflow-y: auto;
    background: radial-gradient(circle at 80% 0%, #17306055 0, transparent 35%), #090d16;
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

  .dashboard-header { display: flex; justify-content: space-between; align-items: flex-start; gap: 24px; margin: 6px 0 24px; }
  .dashboard-header h1 { margin: 2px 0 6px; font-size: 30px; letter-spacing: -.035em; }
  .eyebrow { margin: 0; color: #60a5fa; font-size: 11px; font-weight: 750; text-transform: uppercase; letter-spacing: .13em; }
  .subtitle { margin: 0; color: #8491a7; }
  .dashboard-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 8px; }
  .summary-grid { display: grid; grid-template-columns: repeat(4, minmax(130px, 1fr)); gap: 12px; margin-bottom: 18px; }
  .summary-card { padding: 17px 18px; border: 1px solid #263247; border-radius: 12px; background: linear-gradient(145deg, #151e2e, #101725); box-shadow: 0 14px 35px #0004; }
  .summary-card span { color: #8491a7; font-size: 12px; }
  .summary-card strong { display: block; margin-top: 8px; font-size: 27px; }
  .summary-card.healthy strong { color: #63d997; }
  .summary-card.updates strong { color: #f7bd63; }
  .panel { border: 1px solid #263247; border-radius: 14px; background: #111827dd; overflow: hidden; box-shadow: 0 18px 50px #0005; }
  .panel-title { display: flex; align-items: baseline; gap: 10px; padding: 18px 20px 2px; }
  .panel-title h2 { margin: 0; font-size: 17px; }
  .panel-title span, .muted { color: #718096; font-size: 11px; }
  .container-table { margin-top: 10px; }
  .container-table tbody tr:hover { background: #192235; }
  .image-cell { max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .link-button { background: transparent; border: 0; color: #dcecff; font-weight: 650; padding: 0; }
  .link-button:hover { color: #60a5fa; background: transparent; }
  .row-actions { text-align: right; white-space: nowrap; }
  .row-actions button { margin-left: 6px; }
  .empty-state { text-align: center; padding: 55px 20px; color: #8491a7; }
  .empty-state h2 { color: #d9e2f0; margin: 8px 0; }
  .empty-state p { margin: 0 auto; max-width: 440px; }
  .empty-icon { font-size: 38px; color: #3b82f6; }
  .empty-state.compact { padding: 25px 15px; }

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

  .badge.version-pinned_by_compose {
    background: #25264a;
    color: #a9a7ff;
  }

  .badge.version-unknown {
    background: #2a2e36;
    color: #8b92a1;
  }

  button {
    background: #1a2435;
    border: 1px solid #314057;
    color: #e7e9ec;
    padding: 6px 10px;
    border-radius: 7px;
    cursor: pointer;
    font-size: 12px;
  }

  button:hover {
    background: #243149;
    border-color: #49617f;
  }

  button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  button.danger {
    color: #e07070;
  }

  button.primary {
    background: linear-gradient(135deg, #2563eb, #3b82f6);
    border-color: #4b8df7;
    box-shadow: 0 5px 16px #2563eb33;
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
    background: #111827;
    border: 1px solid #33445e;
    border-radius: 14px;
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

  .maintenance-modal {
    width: 460px;
  }

  .maintenance-list {
    list-style: none;
    padding: 0;
    margin: 8px 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .maintenance-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
    padding: 8px;
    border: 1px solid #2a2e36;
    border-radius: 8px;
  }

  .maintenance-label {
    font-weight: 600;
    font-size: 13px;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .maintenance-desc {
    font-size: 11px;
    color: #8b92a1;
    margin-top: 2px;
    max-width: 280px;
  }

  .risk-badge {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 8px;
    background: #3d1c1c;
    color: #e07070;
    font-weight: normal;
  }

  .maintenance-output {
    background: #14161a;
    border: 1px solid #2a2e36;
    border-radius: 6px;
    padding: 8px;
    font-size: 11px;
    max-height: 140px;
    overflow-y: auto;
    white-space: pre-wrap;
    font-family: ui-monospace, monospace;
    margin: 4px 0;
  }

  .scan-modal { width: 480px; }
  .scanning { display: flex; align-items: center; gap: 10px; padding: 28px 4px; color: #9aacbf; }
  .spinner { width: 15px; height: 15px; border: 2px solid #33445e; border-top-color: #60a5fa; border-radius: 50%; animation: spin .8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .discovery-list { list-style: none; padding: 0; margin: 10px 0; }
  .discovery-list li { display: flex; justify-content: space-between; align-items: center; padding: 12px; border: 1px solid #2c3a51; border-radius: 9px; margin-bottom: 8px; }
  .discovery-list li div { display: flex; flex-direction: column; gap: 3px; }
  .discovery-list span { color: #8491a7; font-size: 11px; }
  @media (max-width: 900px) { .summary-grid { grid-template-columns: repeat(2, 1fr); } .dashboard-header { flex-direction: column; } }
</style>
