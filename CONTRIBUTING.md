# Contributions welcome

Thanks for helping improve Vibeking. Bug reports, focused fixes, translations, and documentation are welcome.

## Report an issue

Search [existing issues](https://github.com/bblurock/vibeking/issues) first. Include the Vibeking version, macOS version, Mac chip, recognition engine, steps to reproduce, and what you expected. A short synthetic example is more useful than a private recording. Remove API keys, transcripts, names, and other private details from logs and screenshots.

Use [SECURITY.md](SECURITY.md) for vulnerabilities.

## Make a change

1. Fork the repository and create a branch for one focused change.
2. Follow [Development](docs/development.md) to run the desktop app.
3. Add or update tests when behavior changes; keep translations aligned when changing UI text.
4. Run `pnpm build` and the Rust tests listed in the development guide. Describe relevant manual checks in your pull request.
5. Explain the problem and resulting behavior, with screenshots for visible changes. Call out any checks you could not run.

For larger changes, open an issue to discuss the approach first. Keep dependency upgrades separate from unrelated product work where practical.

By submitting a contribution, you agree that it can be distributed under the project's MIT license. Only contribute material you have the right to share, and preserve third-party notices. Treat other contributors with respect and keep feedback focused on the work.
