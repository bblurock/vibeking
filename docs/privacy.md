# Privacy

This describes the source and public build of Vibeking 0.1.0.

## Processing and network access

| Choice | What leaves your Mac |
| --- | --- |
| Local recognition | Audio is processed locally after model setup. Initial downloads contact the model/package hosts. |
| Cloud recognition | Recorded audio and recognition options are sent to the provider you choose. Hotwords and optional screen-context candidates may be included where supported. |
| Local refinement | Text is sent to the local endpoint you configure. Verify that endpoint's own behavior. |
| Cloud refinement | Transcript text and the selected refinement instruction are sent to the configured provider. |
| Feedback | Opens GitHub Issues in your browser. No logs or transcripts are attached automatically. Anything you submit there is public. |

The app has no account requirement, analytics service, automatic crash upload, or built-in update check. It does not proxy cloud requests through a Vibeking service. Provider privacy policies and billing apply when you use their APIs. Model and Python setup can contact GitHub, Hugging Face, Python package indexes, and model hosts.

## Local data

- Settings, provider API keys, custom vocabulary, and learned corrections are stored in a local JSON settings file using Tauri Store. **The keys are not protected by macOS Keychain.** Protect your Mac account and avoid sharing the settings file.
- Completed transcripts are saved in the local SQLite history database. You can clear History in the app. Database deletion is not a promise of forensic erasure, and backups may retain older copies.
- Diagnostic logs can contain transcript fragments, app names, errors, and local file paths. Inspect and redact them before sharing.
- Local model weights and Gemma's Python environment occupy disk space. Removing the app does not necessarily remove these caches.

App data normally lives under `~/Library/Application Support/com.vibeking.app/`; logs normally live under `~/Library/Logs/com.vibeking.app/`. Engine-managed caches can use additional locations.

## Optional context and learning

Screen context reads accessible text from the focused app; an optional screenshot/OCR fallback needs Screen Recording permission. The app extracts candidate words to improve recognition. When using a cloud engine, supported requests can include those candidates. Disable screen context if you do not want nearby text used.

Correction learning observes supported edits after insertion and can save replacements to the local dictionary. Review or remove learned corrections in the app and turn learning off if you prefer.

## Reporting a security problem

Please use the private contact in [SECURITY.md](../SECURITY.md), rather than posting sensitive information in a public issue.
