# Running MagicAIBot on Pelican Panel

This document covers everything needed to deploy MagicAIBot in a Pelican
Panel server backed by Docker, using the Rust "yolk" image. It assumes a
fresh server with nothing pre-installed beyond the image itself.

## Required Docker image

```
ghcr.io/parkervcp/yolks:rust_latest
```

No other image, and no modifications to this image, are required. Everything
below works by controlling *where* Rust/Cargo write, not by changing what's
baked into the image.

## Required working directory

`/home/container` -- Pelican's standard per-server directory, and the only
part of the container filesystem that's writable and persists across
restarts. The repository must be cloned (or the Egg's install step must
check it out) directly into this directory, so that e.g.
`/home/container/Cargo.toml` and `/home/container/start.sh` exist at the
container's working directory root.

A plain `git clone` is sufficient -- there is no submodule to initialize.
`vendor/azalea` is committed directly into this repository (see "Why Azalea
isn't a git dependency or submodule" below), so nothing beyond the clone
itself is required to get a complete, buildable source tree.

## Pelican startup command

Set the Egg's **Startup Command** to:

```
bash start.sh
```

That's the entire startup command -- copy-paste it as-is. All of the actual
logic (toolchain bootstrap, config first-run handling, build, run) lives in
[`start.sh`](start.sh), committed at the repository root, so it ships with
every clone and never needs to be re-typed into the panel.

Do not point Pelican's separate one-time "Installation" step at anything
custom for this project. `start.sh` is intentionally idempotent and handles
first-run provisioning itself (see below), so the *same* Startup Command
works identically on the very first boot and on every restart after --
there's nothing that needs to happen in a separate install phase.

## Required environment variables

`start.sh` sets these itself at the top of the script, so **no manual
configuration in the Pelican panel is required**:

```bash
RUSTUP_HOME=/home/container/.rustup
CARGO_HOME=/home/container/.cargo
PATH=/home/container/.cargo/bin:$PATH
```

If you'd rather have them visible as Egg Variables (e.g. for consistency
with a custom console command, or so they show up when you `exec` into the
container manually), you can additionally define `RUSTUP_HOME` and
`CARGO_HOME` as Egg environment variables with the values above -- `start.sh`
will simply re-export the same values, so this is redundant but harmless.

## How Rust/Cargo persistence works

`ghcr.io/parkervcp/yolks:rust_latest` bakes a Rust toolchain into
`/usr/local/rustup` and `/usr/local/cargo` at image-build time, owned by
`root`. The actual server process, however, runs as the unprivileged
`container` user Pelican/Wings always uses, and only `/home/container` is
writable at runtime -- `/usr/local/*` is effectively read-only to that user.
Any attempt to write there (installing a toolchain, `cargo install`, even
some of rustup's own bookkeeping) fails with:

```
error: Read-only file system (os error 30) at path "/usr/local/cargo/bin/..."
```

`start.sh` avoids this by redirecting both Rustup's and Cargo's home
directories under `/home/container` *before* invoking either, so nothing
ever touches `/usr/local`:

- **First boot**: `$CARGO_HOME/bin/cargo` doesn't exist yet, so `start.sh`
  downloads and runs the official `rustup.rs` installer non-interactively,
  installing into `/home/container/.rustup` / `/home/container/.cargo`
  (minimal profile, no default toolchain -- the project's own
  `rust-toolchain.toml` pin is installed as a separate, explicit step right
  after). This is the slow step -- expect it to take a couple of minutes,
  plus however long the subsequent first `cargo build --release` takes
  (several minutes, since every dependency -- including all of vendored
  Azalea -- compiles from scratch).
- **Every restart after that**: `$CARGO_HOME/bin/cargo` already exists (it's
  under `/home/container`, which persists), so the entire bootstrap block is
  skipped -- just an `[ -x ... ]` check, no download, no network call. The
  toolchain-install check works the same way, keyed off `rustup toolchain
  list`. `cargo build --release` on an unchanged source tree is a fast no-op
  (Cargo's own incremental `target/` cache also persists under
  `/home/container`).

This is exactly what point 3 of the deployment requirements asks for: never
blindly reinstall the toolchain, only do it when it's actually missing.

## First-start behavior

On a completely fresh `/home/container` (nothing but the git clone):

1. `start.sh` notices no cargo under `$CARGO_HOME` and bootstraps rustup
   there (see above).
2. It reads `rust-toolchain.toml` (currently pins the `nightly` channel) and
   installs that toolchain, since it isn't present yet either.
3. It notices `config.toml` doesn't exist (it's gitignored -- only
   `config.toml.example` is committed, since real configs hold server/account
   details) and copies `config.toml.example` to `config.toml` so the bot has
   something valid to load instead of crashing on a missing file. The copied
   defaults (`127.0.0.1:25565`, username `MagicBot`, offline account mode)
   are enough to pass config validation and boot, but **will not connect to
   anything real** until you edit `config.toml` (via Pelican's file manager,
   or `docker exec`) with your actual server address and account details,
   then restart the server from the panel.
4. It runs `cargo build --release` (slow, first time only in practice) and
   then execs the resulting `target/release/magic_ai_bot` binary.

Every subsequent restart skips steps 1-2 (already provisioned) and step 3
(config.toml already exists, never overwritten), and step 4's build is fast.

## Why Azalea isn't a git dependency or submodule

`Cargo.toml` depends on Azalea via local path dependencies:

```toml
azalea = { path = "vendor/azalea/azalea", ... }
azalea-client = { path = "vendor/azalea/azalea-client" }
azalea-inventory = { path = "vendor/azalea/azalea-inventory" }
```

`vendor/azalea` used to be registered as a git submodule with no
`.gitmodules` entry ever committed -- the root cause of both:

```
fatal: No url found for submodule path 'vendor/azalea' in .gitmodules
failed to read '/home/container/vendor/azalea/azalea/Cargo.toml': No such file or directory
```

on a fresh clone, since Git had a dangling submodule pointer with nothing
telling it what to clone.

The straightforward fix would be a real git dependency pinned to a tag/rev
of upstream `azalea-rs/azalea` -- but this project's vendored copy carries
substantial local modifications on top of upstream (pathfinder
pillar/bridge/staircase building moves, tool-selection and scaffold-material
policy, and interact/inventory plugin changes) that the bot's movement,
combat, and interaction systems depend on directly and that don't exist in
upstream Azalea. Depending on upstream would compile, but silently remove
those features.

Instead, `vendor/azalea`'s source tree is committed directly into this
repository as ordinary tracked files (no nested `.git`, no submodule). A
plain `git clone` now produces a complete, buildable tree with zero extra
steps -- satisfying the "no `git submodule update --init --recursive`"
requirement -- while keeping every custom modification intact. `target/` and
other build artifacts under `vendor/azalea/` are still excluded via the
top-level `.gitignore`'s existing `target/` rule.

## Troubleshooting

**`error: Read-only file system (os error 30)` mentioning `/usr/local/...`**
Something is running `cargo`/`rustup` without `$RUSTUP_HOME`/`$CARGO_HOME`
set to the `/home/container` paths -- almost always means a command was run
outside of `start.sh` (e.g. a manual console command). Prefix any manual
command with:
```bash
RUSTUP_HOME=/home/container/.rustup CARGO_HOME=/home/container/.cargo PATH=/home/container/.cargo/bin:$PATH <your command>
```

**`fatal: No url found for submodule ...` / missing `vendor/azalea/azalea/Cargo.toml`**
Should no longer be possible -- `vendor/azalea` is committed directly, not a
submodule. If you see this, the checkout is stale or truncated; re-clone.

**Build fails partway through, or `target/` looks corrupted after an
interrupted build**
Safe to delete and let it rebuild:
```bash
RUSTUP_HOME=/home/container/.rustup CARGO_HOME=/home/container/.cargo PATH=/home/container/.cargo/bin:$PATH rm -rf target && cargo build --release
```
This costs a full rebuild (several minutes) but nothing else -- Cargo.lock
and the toolchain install are untouched.

**Toolchain channel mismatch / `error: toolchain 'X' is not installed`**
`start.sh` reads the required channel from `rust-toolchain.toml` at the repo
root every startup, so if that file's `channel` value changes upstream, the
next restart installs the new one automatically (the old one is left
installed under `$RUSTUP_HOME`, harmlessly unused, rather than being
removed).

**`curl: command not found` during first-boot bootstrap**
`start.sh` needs `curl` to fetch the rustup installer. This should be
present in `ghcr.io/parkervcp/yolks:rust_latest`; if it's genuinely missing,
that's an image problem, not a MagicAIBot one -- confirm you're using the
exact image tag above.

**Bot starts but can't connect / immediately reconnect-loops**
Almost always `config.toml` still has the copied-over placeholder values
(`127.0.0.1:25565`, `MagicBot`). Edit `config.toml` with real values and
restart the server.
