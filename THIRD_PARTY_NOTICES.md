# Third-party notices

Vibeking's own code and original artwork are licensed under MIT. Third-party components retain their own licenses. This file and the `licenses/` directory are included in packaged apps.

## Adapted and vendored source

| Component | Use and provenance | License |
| --- | --- | --- |
| Whispering | Clipboard insertion in `src-tauri/src/insert.rs` and provider patterns in `src-tauri/src/stt.rs`, adapted from `apps/whispering` in EpicenterHQ/epicenter. | [MIT, Copyright (c) 2023–2026 Braden Wong](licenses/Whispering-MIT.txt) |
| fluidaudio-rs 0.14.1 | Source in `vendor/fluidaudio-rs`, based on upstream commit `f9f7d10c840eac33451093bdd4d6c58ba3d08f47`. Modified token timing and streaming FFI; [patch notes](vendor/fluidaudio-rs/FORK_NOTES.md). | [MIT](licenses/fluidaudio-rs-MIT.txt), as declared in upstream package metadata. The upstream package did not contain a standalone license file; the standard MIT text is reproduced here with contributor attribution. |
| FluidAudio 0.14.1 | Swift dependency of the vendored bridge, pinned to `d302273d49ef4d8914b27f20d342be482e8810f1` in `Package.resolved`. | [Apache 2.0](licenses/FluidAudio-Apache-2.0.txt) |
| uv 0.11.23 | Unmodified Apple Silicon executable downloaded by `scripts/fetch-uv.sh` and bundled for optional Python/model setup. | [MIT](licenses/uv-MIT.txt), one of uv's available licenses |

Whispering's historical MIT grant is preserved from [this revision](https://github.com/EpicenterHQ/epicenter/blob/2a843e11dcfc821e10462b1e9015968fd1d256a9/apps/whispering/LICENSE). Epicenter's current root license does not replace the historical notice for the adapted code.

Upstream sources: [fluidaudio-rs](https://github.com/FluidInference/fluidaudio-rs/tree/f9f7d10c840eac33451093bdd4d6c58ba3d08f47), [FluidAudio](https://github.com/FluidInference/FluidAudio/tree/d302273d49ef4d8914b27f20d342be482e8810f1), [uv](https://github.com/astral-sh/uv/tree/0.11.23).

## Other dependencies

[Collected dependency notices](licenses/DEPENDENCIES.txt) and the supplementary upstream texts in `licenses/` accompany the binary. Unmodified MPL-2.0 dependency source is available through the exact version download links in that file.

Rust and JavaScript dependencies are recorded in `src-tauri/Cargo.lock` and `pnpm-lock.yaml`; their published packages carry their own license notices. These include Tauri, React, whisper.cpp/whisper-rs, and their dependencies. Python dependencies for optional Gemma setup are pinned with hashes in `src-tauri/resources/gemma-requirements.txt` and installed on demand.

## Models and services

Model weights are downloaded separately and are not distributed in this repository or app bundle. Whisper, Parakeet, Qwen3, Gemma, and any model used for refinement have separate model licenses and terms. The MIT license for Vibeking does not relicense them. Cloud services have their own terms and charges.

## Media

The app icon and screenshots were created for Vibeking and reused from [Benson Lu's project page](https://www.benson.lu/project/vibeking). The completion sound is an original synthesized tone, reproducible with `python3 scripts/generate-sound.py`. These project-owned assets are included under MIT.
