<p align="center"><img src="docs/images/app-icon.png" width="112" alt="Vibeking app icon"></p>

<h1 align="center">Vibeking</h1>
<p align="center">Speak. Put words anywhere.</p>
<p align="center">Voice typing for Mac, with local speech recognition and optional writing refinement.</p>
<p align="center"><a href="https://github.com/bblurock/vibeking/releases/latest">Download for Mac</a> · <a href="docs/getting-started.md">Getting started</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="https://www.benson.lu/project/vibeking">Project page</a></p>

<p align="center"><strong>Apple Silicon · macOS 14+ · Free · MIT</strong></p>
<p align="center"><img src="docs/images/floating-bar.png" width="440" alt="Vibeking’s floating dictation bar, shown with Chinese labels"></p>

## Say it, then keep going

- **Write where you work.** Hold your recording shortcut, speak, and release to insert the result into the focused app. Tap for hands-free recording; press Escape to cancel.
- **Choose where speech is processed.** Run Whisper, Parakeet, Qwen3, or Gemma on your Mac, or connect a supported cloud provider with your own API key.
- **Give your words a shape.** Keep the direct transcript, or use editable modes to polish a sentence, organize notes, draft an email, or translate.

A floating dictation bar, transcription history, custom vocabulary, and optional correction learning keep everyday controls close. The interface supports English, Traditional Chinese, and Simplified Chinese.

## Get started

1. Download the Apple Silicon `.dmg` from [Releases](https://github.com/bblurock/vibeking/releases/latest), open it, and drag **Vibeking** into **Applications**.
2. Open Vibeking and follow setup. Allow **Microphone** for recording and **Accessibility** for the shortcut and text insertion. Download a local recognition model, or configure a cloud provider.
3. Focus a text field, hold your configured shortcut, say a short sentence, and release. Start with refinement off so you can check recognition first.

Local models need an initial internet download and enough disk space and memory. Gemma also installs an app-managed Python environment; no separate Python setup is needed for the downloaded app. See [setup and troubleshooting](docs/getting-started.md).

## A few details

<details>
<summary><strong>Your shortcut and microphone</strong></summary>

Choose your input device and recording shortcut, or keep the small dictation bar on screen.

<img src="docs/images/dictation.png" width="600" alt="Recording shortcut, microphone, cancellation key, and floating bar settings">
</details>

<details>
<summary><strong>Editable writing modes</strong></summary>

Choose a mode and adjust its instruction to fit your writing. Refinement uses the local or cloud language-model provider you configure.

<img src="docs/images/refinement.png" width="600" alt="Notes refinement mode with its editable instruction">
</details>

<details>
<summary><strong>Recognition engines</strong></summary>

Each local engine has different language coverage and resource needs. Cloud options include Deepgram, Groq, OpenAI, and ElevenLabs. Speed and accuracy depend on the model, language, recording, and hardware; the screenshot’s labels are UI guidance, not comparative benchmarks.

<img src="docs/images/engines.png" width="600" alt="Local recognition engines and their download status">
</details>

These are screenshots of the app, reused from its [project page](https://www.benson.lu/project/vibeking).

Qwen3 requires macOS 15 or later, even though the app itself supports macOS 14.

## Privacy and cost

Local recognition processes audio on your Mac after setup. Cloud recognition sends audio to your chosen provider; cloud refinement sends text to its configured provider. Optional screen context can add words from the focused app to recognition requests. Turn it off if you do not want that context used.

Transcription history and settings are stored locally. **API keys are stored in the local settings file, not macOS Keychain.** Logs can contain diagnostic details and transcript fragments. The public build has no automatic log upload or analytics; its feedback button opens GitHub Issues. Updates are downloaded manually from Releases.

Vibeking is free. Cloud providers may charge for API use, and downloaded models retain their own terms. Read [Privacy](docs/privacy.md) for storage, permissions, and network behavior.

## Build from source

Use an Apple Silicon Mac with macOS 14+, **Xcode with its command-line tools selected**, Rust stable, Node.js 22.12+, and pnpm 10.20.0.

```sh
git clone https://github.com/bblurock/vibeking.git
cd vibeking
pnpm install --frozen-lockfile
bash scripts/fetch-uv.sh
pnpm tauri dev
```

To make a local app bundle:

```sh
pnpm tauri build --bundles app
```

Contributor builds use ad-hoc signing. You do not need the maintainer’s certificates, private repositories, environment files, or model weights to compile. [Development guide](docs/development.md) covers tests, architecture, and release signing.

## Contributions welcome

Bug reports, documentation improvements, translations, and focused pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for the workflow. For a vulnerability, follow [SECURITY.md](SECURITY.md).

### Current limitations

- Apple Silicon Macs only; Intel, Windows, and Linux are unsupported.
- Accessibility permissions and the target app affect automatic insertion. Some protected fields and custom editors may need a manual paste.
- Recognition and refinement can make mistakes. Check names, numbers, and important wording before sending.
- Larger local models use several gigabytes of memory. Model setup and first use can take time.
- There is no built-in updater yet; use Releases for new versions.

## License and acknowledgements

[MIT](LICENSE) © Benson Lu. Includes source adapted from Whispering and a vendored fluidaudio-rs bridge. See [third-party notices](THIRD_PARTY_NOTICES.md) for their licenses and the separate terms for dependencies and models.

Made by [Benson Lu](https://www.benson.lu).
