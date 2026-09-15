#!/usr/bin/env bash
#
# Fetch the pinned `uv` binary and place it where Tauri expects an externalBin
# sidecar: src-tauri/binaries/uv-<target-triple>. The binary is gitignored and
# fetched at build time (and on demand) so we don't carry a ~35 MB blob in git.
#
# uv is what lets Gemma's Python setup stop depending on the user's machine: at
# `setup()` time the app drives this binary to install a managed CPython 3.11
# and the pinned mlx-vlm deps into the app's own venv. See
# src-tauri/src/gemma_server.rs and plans (uv sidecar).
#
# Idempotent: skips the download when the correct binary is already present.
set -euo pipefail

# Pin the uv version so every build/install is reproducible. Bump deliberately.
UV_VERSION="0.11.23"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST_DIR="$ROOT/src-tauri/binaries"

# The Apple-Silicon triple is the only one we ship: Gemma (MLX) is
# Apple-Silicon-only, and uv is only used by the Gemma setup path.
TRIPLE="aarch64-apple-darwin"
ASSET="uv-${TRIPLE}.tar.gz"
DEST="$DEST_DIR/uv-${TRIPLE}"

mkdir -p "$DEST_DIR"

# Skip if we already have this exact uv version.
if [[ -x "$DEST" ]] && "$DEST" --version 2>/dev/null | grep -q "uv ${UV_VERSION}"; then
  echo "[fetch-uv] uv ${UV_VERSION} already present at $DEST"
  exit 0
fi

BASE="https://github.com/astral-sh/uv/releases/download/${UV_VERSION}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "[fetch-uv] downloading uv ${UV_VERSION} (${TRIPLE})"
curl -fsSL "$BASE/$ASSET" -o "$TMP/$ASSET"
curl -fsSL "$BASE/$ASSET.sha256" -o "$TMP/$ASSET.sha256"

echo "[fetch-uv] verifying checksum"
EXPECTED="$(awk '{print $1}' "$TMP/$ASSET.sha256")"
ACTUAL="$(shasum -a 256 "$TMP/$ASSET" | awk '{print $1}')"
if [[ "$EXPECTED" != "$ACTUAL" ]]; then
  echo "[fetch-uv] CHECKSUM MISMATCH" >&2
  echo "  expected: $EXPECTED" >&2
  echo "  actual:   $ACTUAL" >&2
  exit 1
fi

echo "[fetch-uv] extracting"
tar -xzf "$TMP/$ASSET" -C "$TMP"
# The tarball extracts to uv-<triple>/uv (+ uvx); we only need uv.
cp "$TMP/uv-${TRIPLE}/uv" "$DEST"
chmod +x "$DEST"

echo "[fetch-uv] installed -> $DEST"
"$DEST" --version
