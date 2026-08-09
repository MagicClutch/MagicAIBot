#!/usr/bin/env bash
# Pelican/Docker startup script for MagicAIBot (ghcr.io/parkervcp/yolks:rust_latest).
#
# Everything Cargo/Rustup ever writes goes under /home/container -- the one
# writable, persistent volume Pelican mounts for this container. The
# yolks:rust_latest image bakes its own toolchain into /usr/local/{cargo,rustup},
# owned by root; the actual process runs as the unprivileged `container` user,
# so anything touching /usr/local fails with "Read-only file system (os error
# 30)". Redirecting both homes here, before rustup/cargo are ever invoked,
# avoids that path entirely rather than trying to make /usr/local writable.
#
# See PELICAN.md for the full write-up (env vars, Pelican startup command,
# first-run behavior, troubleshooting).
set -euo pipefail

cd /home/container

export RUSTUP_HOME="/home/container/.rustup"
export CARGO_HOME="/home/container/.cargo"
export PATH="$CARGO_HOME/bin:$PATH"

# --- Toolchain bootstrap (once) ---------------------------------------------
# Guarded on the cargo binary actually existing under $CARGO_HOME -- on every
# restart after the first, this whole block is a single `[ -x ... ]` check,
# no download, no network call.
if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
    echo "==> No cargo found under $CARGO_HOME -- installing rustup there..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
        -y \
        --no-modify-path \
        --default-toolchain none \
        --profile minimal
fi

# rust-toolchain.toml at the repo root pins the toolchain this project
# actually needs (currently nightly) -- read it rather than hardcoding it
# here so this script never drifts out of sync with the pin. Installed once
# and cached under $RUSTUP_HOME the same way the bootstrap above is.
REQUIRED_CHANNEL="$(grep -m1 '^channel' rust-toolchain.toml | sed -E 's/.*"(.*)".*/\1/')"
REQUIRED_CHANNEL="${REQUIRED_CHANNEL:-nightly}"
if ! rustup toolchain list | grep -q "^${REQUIRED_CHANNEL}"; then
    echo "==> Installing Rust toolchain '$REQUIRED_CHANNEL'..."
    rustup toolchain install "$REQUIRED_CHANNEL" --profile minimal
fi

# --- First-run config --------------------------------------------------------
# config.toml is gitignored (it holds real server/account details) and never
# shipped in the repo -- only config.toml.example is. Without this, a fresh
# container's first boot would crash immediately in Config::load with a
# missing-file error instead of coming up with editable placeholder values.
if [ ! -f "config.toml" ]; then
    echo "==> No config.toml found -- copying config.toml.example."
    echo "    Edit config.toml with your real server/account details, then restart."
    cp config.toml.example config.toml
fi

# --- Build + run --------------------------------------------------------------
# `cargo build` on an unchanged source tree is a fast no-op (target/ persists
# under /home/container across restarts, same as $CARGO_HOME/$RUSTUP_HOME) --
# this is not a "rebuild from scratch every restart" step.
echo "==> Building (release)..."
cargo build --release
echo "==> Starting MagicAIBot..."
exec ./target/release/magic_ai_bot
