#!/usr/bin/env bash
# Build and notarize an official Apple Silicon release. Credentials stay in the environment.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
: "${APPLE_SIGNING_IDENTITY:?Set your Developer ID Application identity}"
: "${APPLE_ID:?Set your Apple ID}"
: "${APPLE_PASSWORD:?Set an app-specific password}"
: "${APPLE_TEAM_ID:?Set your Apple Developer Team ID}"
export APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
export PATH="/usr/bin:$PATH"
config_dir="$(mktemp -d -t vibeking-signing)"
config_file="$config_dir/config.json"
trap 'rm -rf "$config_dir"' EXIT
export APPLE_SIGNING_IDENTITY
node -e 'process.stdout.write(JSON.stringify({bundle:{macOS:{signingIdentity:process.env.APPLE_SIGNING_IDENTITY}}}))' > "$config_file"
bash scripts/fetch-uv.sh
pnpm tauri build --config "$config_file" --bundles app,dmg
# Tauri notarizes the app. Submit and staple the disk image as well.
artifact_dir="${CARGO_TARGET_DIR:-src-tauri/target}/release/bundle/dmg"
shopt -s nullglob
images=("$artifact_dir"/*.dmg)
[[ ${#images[@]} -eq 1 ]] || { echo "Expected exactly one DMG in $artifact_dir" >&2; exit 1; }
dmg="${images[0]}"
xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
shasum -a 256 "$dmg"
