# Fork notes — vibeking-specific additions

This is a **fork** of [`fluidaudio-rs`](https://github.com/FluidInference/fluidaudio-rs)
**v0.14.1**, vendored from crates.io.

- Upstream base commit (from `.cargo_vcs_info.json`): `f9f7d10c840eac33451093bdd4d6c58ba3d08f47`
- Consumed by Vibeking through `../../src-tauri/Cargo.toml`. The relative path keeps a fresh clone self-contained.

The fork carries patches on top of upstream v0.14.1 that are not (yet) upstreamed.
This file documents them so they can be re-derived if the fork is ever lost or has
to be re-applied to a newer upstream.

---

## 1. `transcribe_samples_timed` — batch transcription WITH per-token timings

**Why:** vibeking's Parakeet real-time live preview re-decodes a short trailing
window every ~200 ms and needs each decoded token's `[start, end]` audio time to
drive the frozen-head / live-tail `TailStitcher` (commit settled text, keep the
last ~1 s revising). The stock `transcribe_samples` drops the timings the decoder
already computes; this addition surfaces them.

**Return shape:** `(text, token_timings_json)` where the JSON is an array of
`{"t": token, "s": startSec, "e": endSec}`.

Implemented across all three layers:

### Swift — `swift/FluidAudioBridge.swift`
- `func transcribeSamplesTimed(_ samples: [Float]) throws -> (String, String)`
  — runs `asrManager.transcribe(samples, decoderState:&)` (same path as
  `transcribeSamples`), then serializes `result.tokenTimings`
  (`{token, startTime, endTime}`) to the JSON array above. Returns `"[]"` when no
  timings are present.
- `@_cdecl("fluidaudio_transcribe_samples_timed")` C export:
  ```
  (ptr, samples: *const Float, sampleCount: UInt32,
   outText: **CChar, outTimingsJson: **CChar) -> Int32   // 0 = ok, -1 = err
  ```
  Both out-strings are `strdup`'d and must be freed via `fluidaudio_free_string`.

### Rust FFI — `src/ffi/bridge.rs`
- `extern` decl `fluidaudio_transcribe_samples_timed(...)` matching the C export.
- Safe wrapper:
  `pub fn transcribe_samples_timed(&self, samples: &[f32]) -> Result<(String, String), String>`
  — fills two `*mut i8` out-pointers, copies each to an owned `String`, frees the
  C strings, returns `Err("Timed transcription failed")` on non-zero status. JSON
  defaults to `"[]"` if the pointer is null.

### Rust public API — `src/lib.rs`
- `pub fn transcribe_samples_timed(&self, samples: &[f32]) -> Result<(String, String), FluidAudioError>`
  — thin map over the bridge wrapper.

**Example:** `examples/transcribe_samples.rs` demonstrates the plain
`transcribe_samples`; the timed variant has the same call shape plus the JSON
return. **Test:** `tests/ffi_bindings.rs` references `transcribe_samples_timed`.

---

## 2. Streaming ASR FFI (Parakeet `SlidingWindow`) — present but UNUSED by vibeking

`fluidaudio_streaming_asr_start / _feed / _finish`,
`fluidaudio_transcribe_file_streaming`, `fluidaudio_is_streaming_asr_available`
(+ the Swift `streamingLiveConfig` / consumer machinery, and the Rust
`local_parakeet::streaming_*` wrappers on the vibeking side).

This was the original streaming approach. vibeking moved to the windowed
batch-decode + `TailStitcher` design (above) and **no longer calls these**. They
are kept for now but are dead from vibeking's perspective and safe to delete in a
cleanup pass. The retained streaming implementation is in `swift/FluidAudioBridge.swift`.

---

## Re-deriving these patches against a newer upstream

1. Add the Swift `transcribeSamplesTimed` method + `@_cdecl` export next to the
   existing `transcribeSamples` / `fluidaudio_transcribe_samples` in
   `swift/FluidAudioBridge.swift`, reading `ASRResult.tokenTimings`.
2. Mirror the `extern` decl + safe wrapper in `src/ffi/bridge.rs`.
3. Add the public method in `src/lib.rs`.
4. `cargo check` (and rebuild the Swift static lib via `build.rs` — see
   `build.rs` for the build system).
