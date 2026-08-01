# dockman

A Tauri app that manages `docker compose` stacks across every PC on your
LAN over plain SSH — no agent to install anywhere, no tokens to manage.

## How it works

- You already have working SSH access to these machines (that's what the
  `~/.ssh/config` aliases assume — keys, ProxyJump, whatever). dockman
  reuses that as-is; it doesn't touch your SSH config or manage auth.
- Each host you add is just an alias from `~/.ssh/config`. Each host has
  one or more compose-stack directories you tell it about (the folder
  containing `docker-compose.yml`).
- Every action is `ssh <alias> "cd <dir> && docker compose <subcommand>"`
  run via `tokio::process::Command`. That's the entire remote-execution
  layer — no daemon, no custom protocol.
- "Update" runs `docker compose pull && docker compose up -d` in that
  directory, which is the correct way to upgrade a multi-service stack:
  it respects the dependency graph, networks, and volumes as declared in
  the compose file, rather than recreating one container in isolation.
- Status comes from `docker compose ps --format json` (newline-delimited
  JSON, one object per service), parsed and shown per stack.

```
dockman-core   shared types only (HostConfig, ComposeDir, StackStatus, ...)
dockman-gui    the only binary — Tauri app, Svelte frontend + Rust backend
```

## Requirements

- `ssh` on your PATH on the machine running the GUI, with working
  key-based auth to each target (BatchMode is used, so it'll fail fast
  rather than hang on a password prompt).
- `docker` and the `compose` plugin (`docker compose`, not the old
  standalone `docker-compose`) on each **remote** host — nothing needs to
  be installed there beyond that.
- Rust + Node/npm + the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
  on the machine you build the GUI on.

## Building

```bash
# 1. install JS deps (also pulls in the Tauri CLI as a dev dependency)
cd dockman-gui
npm install

# 2. sanity-check the Rust side compiles before involving the CLI/webview
cd src-tauri
cargo check
cd ..

# 3. dev build with hot reload
npm run tauri dev

# 4. release bundle (.AppImage/.deb on Linux, .app/.dmg on macOS)
npm run tauri build
```

If `npm run tauri` itself fails ("command not found" / can't find the
`tauri` binary), the Tauri CLI didn't get installed as a dev dependency —
run `npm install -D @tauri-apps/cli@^2` explicitly and retry.

A placeholder icon (plain rounded-square glyph) ships in
`src-tauri/icons/` so `generate_context!()` and bundling both have
something to embed — `tauri::generate_context!()` hard-requires
`icons/icon.png` to exist even before you get to the bundle step, which
is what the "failed to open icon icon.png" error was. Swap in a real
logo any time with: `npm run tauri icon path/to/a-1024x1024.png` from
inside `dockman-gui`.

If you hit a different error than "can't find src/lib.rs" or a missing
icon, paste it and I'll take a look — Tauri v2's plugin/config surface
has shifted across point releases since my training data, so something
else may have drifted too.

## Using it

1. **Add a host** — the dropdown is populated from `Host` entries in
   `~/.ssh/config` (including one level of `Include`d files). Hit "Test
   connection" before saving if you want to confirm the alias actually
   resolves and key auth works.
2. **Add a stack directory** — the absolute path on that host where
   `docker-compose.yml` lives, e.g. `/home/jim/docker/nextcloud`.
3. Buttons available:
   - Per stack: **Start**, **Restart**, **Stop**, **Update** (pull + up -d)
   - Per host: the refresh icon next to the host name runs **Update** on
     every stack for that machine
   - Global: **Update all** / **Refresh all** in the sidebar run across
     every stack on every host, in parallel
4. **Check for updates** (per stack) compares each running image's
   digest against what the registry currently has for the same tag —
   this is the accurate way to detect "is there something new," since
   Docker tags don't carry semantic version numbers on their own; a
   digest change is the actual signal `docker compose pull` itself acts
   on. Requires `docker buildx` on that remote host (bundled with
   modern Docker Engine and Docker Desktop — worth confirming on
   CachyOS specifically since Arch packaging sometimes splits it out).
   Shows **up to date / update available / unknown** per service;
   "unknown" covers locally-built images with no registry digest,
   missing buildx, private-registry auth issues, etc.
5. Host/stack config persists to `~/.config/dockman/hosts.json` (or the
   platform equivalent) — no secrets stored, since auth lives entirely in
   your existing SSH setup.

## Caveats

- No live streaming of `docker compose` output while a command runs —
  you get the combined stdout/stderr once the ssh call finishes, shown
  in the status line on failure. Fine for pull+up cycles that take a few
  seconds to a minute; would want an event-streaming version
  (`tauri::Emitter` + reading the child process's stdout incrementally)
  if you start running stacks that take much longer to update.
- Runs actions across a host's stacks concurrently, not sequentially —
  fine unless two stacks on the same host compete hard for CPU/disk
  during a simultaneous pull. Easy to cap concurrency (a semaphore, or
  just `.chunks()` the futures) if that becomes a problem.
- `docker compose ps --format json` field names have shifted a bit
  across compose versions (particularly `Ports` vs `Publishers`); the
  parser here handles both but hasn't been tested against your actual
  compose version on either machine.
- The version check hashes the raw registry manifest on the remote host
  itself (not locally) specifically to avoid any risk of the digest
  bytes getting mangled on the way through ssh's text output — but it's
  still comparing against `docker image inspect`'s `RepoDigests`, which
  is only populated once an image has actually been pulled by tag at
  least once. A locally-built image, or one loaded via `docker load`,
  will show as "unknown" rather than a false positive/negative.
- No confirmation dialog before Stop/Update — worth adding if a stray
  click on the wrong stack would be annoying.
