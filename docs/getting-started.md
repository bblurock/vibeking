# Getting started

## Install

Vibeking requires an Apple Silicon Mac and macOS 14 or later. Download the `.dmg` from [GitHub Releases](https://github.com/bblurock/vibeking/releases/latest), drag Vibeking to Applications, and open it from there. Official release notes state signing and notarization status; builds you make yourself are ad-hoc signed by default.

## First dictation

1. Allow Microphone and Accessibility when prompted. You can manage both in System Settings → Privacy & Security.
2. Choose a recognition engine. Download a local model and wait for it to become ready, or enter your own cloud API key. Whisper is a useful starting point if you want a smaller multilingual local download.
3. Choose a recording shortcut and check the selected microphone. Leave refinement off for the first attempt.
4. Open a plain text document, focus its text field, hold the shortcut, say a short sentence, and release. A quick tap toggles hands-free recording. Escape cancels.

For optional refinement, configure a local or cloud language-model provider, then choose or edit a mode. Recognition and refinement have separate provider settings.

## Permissions

| Permission | Purpose |
| --- | --- |
| Microphone | Record speech. |
| Accessibility | Global shortcuts, automatic text insertion, and optional focused-app context/correction learning. |
| Screen Recording | Optional OCR fallback for screen context. Basic dictation does not require it. |

Read [Privacy](privacy.md) before enabling context features or sharing logs.

## Models

Download local models from the engine picker. Downloads may be several gigabytes and first startup can be slower than later use. Gemma installs an app-managed Python 3.11 environment through the bundled uv tool. Model downloads need internet access; model weights are separate from the app download. Language coverage varies by engine. Cloud providers require your own key and may charge for usage.

Qwen3 requires macOS 15 or later, even though the app itself supports macOS 14.

## Troubleshooting

- **No recording:** check Microphone permission, selected input device, and whether the device works in another recording app. Try the built-in microphone if a Bluetooth device has disconnected.
- **Text appears in history but not the target app:** check Accessibility permission, focus a normal text field, and try again. Copy the result from History if the editor does not accept automatic insertion.
- **Shortcut does nothing:** choose a shortcut that does not conflict with macOS or another app. Recheck Accessibility after replacing an ad-hoc development build.
- **Model setup fails:** check network access and disk space, then retry. Include the engine and error message in a GitHub issue; do not include API keys.
- **Blank window after opening:** quit and reopen Vibeking. If other Mac apps also have audio problems, disconnect Bluetooth audio or restart the Mac. This release moves input-device discovery off the main thread; an unresponsive system audio service may still need a macOS restart. Do not delete your settings as the first step.
- **Unexpected words:** check language/model selection, disable refinement to compare the raw transcript, and check hotwords, screen context, and learned corrections.

## Update or uninstall

Download a newer release, quit Vibeking, and replace the app in Applications. There is no built-in updater in this version. Keep your data unless you explicitly want to reset it.

To uninstall, quit Vibeking and move the app to Trash. Local data can remain in `~/Library/Application Support/com.vibeking.app/`, and logs in `~/Library/Logs/com.vibeking.app/`. Engine-managed model caches may be elsewhere under your user Library or cache directories. Use the engine picker’s clear/remove controls before uninstalling if you want to reclaim those downloads. Removing app data also removes settings, API keys, and history.
