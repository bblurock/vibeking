"""Collect notices from installed locked dependencies before packaging a release.

Run after pnpm install and a native build (which resolves the Swift package).
The output intentionally includes build dependencies as well as runtime ones.
"""
from pathlib import Path
import json
import subprocess

root = Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output([
    'cargo', 'metadata', '--manifest-path', str(root / 'src-tauri/Cargo.toml'),
    '--locked', '--format-version', '1', '--filter-platform', 'aarch64-apple-darwin',
]))
sections = ['THIRD-PARTY DEPENDENCY NOTICES\n\nGenerated from installed, locked packages.\nIncludes build tools as well as runtime dependencies.\nUnmodified MPL-2.0 crate source is available from the exact crates.io version URLs below.\n']
missing = []
def collect(name, version, license_name, directory, source):
    files = sorted(p for p in directory.iterdir() if p.is_file() and p.name.lower().startswith(('license', 'licence', 'copying', 'notice')))
    for subdir in ('LICENSES', 'licenses', 'whisper.cpp'):
        folder = directory / subdir
        if folder.is_dir():
            files.extend(f for f in folder.rglob('*') if f.is_file() and (subdir != 'whisper.cpp' or f.name == 'LICENSE'))
    sections.append('\n' + '=' * 72 + f'\n{name} {version}\nLicense: {license_name}\nSource: {source}\n')
    if not files:
        missing.append(f'{name} {version}')
    for p in files:
        sections.append(f'\n--- {p.name} ---\n' + p.read_text(errors='replace'))

for pkg in sorted(metadata['packages'], key=lambda x: (x['name'], x['version'])):
    if not pkg['source']:
        continue # Project/vendored code is covered by the root notices.
    sections.append('\nPackage authors: ' + ', '.join(pkg.get('authors', [])) + '\n')
    collect(pkg['name'], pkg['version'], pkg['license'], Path(pkg['manifest_path']).parent,
            f"https://crates.io/api/v1/crates/{pkg['name']}/{pkg['version']}/download")
seen = set()
for p in sorted((root / 'node_modules/.pnpm').glob('*/node_modules/**/package.json')):
    # Only package roots; skip fixtures and nested source manifests.
    try:
        pkg = json.loads(p.read_text())
        key = (pkg['name'], pkg['version'])
    except (KeyError, ValueError):
        continue
    if key in seen:
        continue
    seen.add(key)
    collect(*key, pkg.get('license', 'See package notices'), p.parent,
            f'https://www.npmjs.com/package/{key[0]}/v/{key[1]}')
for p in sorted((root / 'src-tauri/target').glob('debug/build/fluidaudio-rs-*/out/swift-build/checkouts/FluidAudio/ThirdPartyLicenses/*')):
    if p.is_file():
        sections.append('\n--- FluidAudio / ' + p.name + ' ---\n' + p.read_text())
(root / 'licenses/DEPENDENCIES.txt').write_text('\n'.join(sections))
print('Wrote licenses/DEPENDENCIES.txt')
print('Packages without a root license file (review metadata/upstream notices):')
print('\n'.join(missing))
