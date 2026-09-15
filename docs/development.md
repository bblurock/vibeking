# Development

## Requirements

- Apple Silicon Mac, macOS 14+.
- Full Xcode, with its license accepted and command-line tools selected in Xcode Settings → Locations. The native build uses Swift, CoreML, and Metal.
- Rust stable with Cargo, Node.js 22.12+, pnpm 10.20.0.
- Internet access for locked dependencies, the pinned uv sidecar, and Swift package resolution. Model downloads are needed to use the engines, but not for unit tests or compilation.

```sh
pnpm install --frozen-lockfile
bash scripts/fetch-uv.sh
pnpm tauri dev
```

`pnpm dev` alone opens the frontend development server. It cannot test native recording, permissions, or insertion; use `pnpm tauri dev` for the desktop app.

## Verification

```sh
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib --tests
pnpm tauri build --bundles app
```

The tests cover audio helpers, device-cache behavior, correction detection, proper-noun extraction, and streaming transcript assembly. They use local synthetic fixtures and do not download model weights or call paid APIs. Native permissions and a real dictation still need a manual smoke test.

## Structure

| Path | Responsibility |
| --- | --- |
| `src/routes/`, `src/components/` | React interface and onboarding. |
| `src/lib/` | Settings, local history, events, and translations. |
| `src-tauri/src/` | Rust audio capture, shortcuts, insertion, recognition, and refinement. |
| `src-tauri/resources/` | Gemma Python sidecar and hashed Python requirements. |
| `vendor/fluidaudio-rs/` | Modified Rust/Swift bridge for local CoreML recognition. |
| `scripts/fetch-uv.sh` | Fetch the pinned Apple Silicon uv binary and verify its published checksum. |
| `docs/`, `licenses/` | User/developer documentation and third-party license notices. |

The FluidAudio bridge is intentionally vendored: Vibeking uses token timings that the base crate does not expose. See its [fork notes](../vendor/fluidaudio-rs/FORK_NOTES.md) before upgrading. Keep Cargo, pnpm, Swift, and Python lock information in version control.

## Signing and releases

The default Tauri configuration uses ad-hoc signing, so contributors can build without an Apple Developer account. macOS may ask for Accessibility permission again after replacing a development build.

For an official signed and notarized release, set the environment variables documented in `.env.example` and run:

```sh
bash scripts/build-release.sh
```

The script requires a Developer ID identity and Apple notarization credentials. It reads the environment only; it does not source local credential files or private service configuration. Do not commit certificates, passwords, API keys, personal recordings, or app data. Release assets belong on GitHub Releases, not in source control.

Before publishing, run the checks above, scan source for secrets, inspect screenshots and dependency notices, verify the signed app and DMG, and smoke-test a first launch and short dictation. Publish the release notes and SHA-256 checksum with the DMG.

## Portable CPU builds

`.cargo/config.toml` passes `cmake/apple-silicon.cmake` to Whisper’s CMake build. It disables host-specific CPU instructions while retaining Metal and Accelerate, so an app built on a newer Mac can run on older Apple Silicon. It also avoids the upstream i8mm feature-probe failure on GitHub’s M1 runners.

If you previously built this checkout before the configuration was added, clear Whisper’s cached native build once:

```sh
cargo clean --manifest-path src-tauri/Cargo.toml -p whisper-rs-sys
cargo clean --manifest-path src-tauri/Cargo.toml --release -p whisper-rs-sys
```

Then repeat the normal tests and build. A fresh clone needs no extra step.
