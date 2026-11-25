"""Writes the licenses of everything in the release AppDir (runs inside the
release container, from build-appimage.sh):

- usr/share/licenses/<component>/ — the license texts;
- usr/share/doc/orbis/THIRD-PARTY.md — what is inside, its version, its
  license and where its source is; a copy goes next to the AppImage.

Where a library came from is found by its file name: Qt's own folder, the
libraries build-deps.sh made in /opt/deps, else the AlmaLinux package that
owns it (rpm -qf), whose license files are copied. The Rust crates are
listed from cargo metadata, with the license files of their vendored
sources.

Usage: python3 -I licenses.py <AppDir> <unpacked Node.js> <orbis version>
"""

import json
import os
import shutil
import subprocess
import sys
import tarfile

APPDIR, NODE_DIR, VERSION = sys.argv[1:4]
VERSIONS = {}
with open('/scripts/versions.sh', encoding='utf-8') as file:
    for line in file:
        if '=' in line and not line.startswith('#'):
            key, value = line.strip().split('=', 1)
            # $NAME in a value is an earlier key, as the shell would read it.
            for name in sorted(VERSIONS, key=len, reverse=True):
                value = value.replace(f'${name}', VERSIONS[name])
            VERSIONS[key] = value
QT = f"/opt/qt/{VERSIONS['QT_VERSION']}/gcc_64"
LICENSES = os.path.join(APPDIR, 'usr/share/licenses')
DOC = os.path.join(APPDIR, 'usr/share/doc/orbis')
WORK = '/tmp/licenses'
LICENSE_NAMES = ('LICENSE', 'LICENCE', 'COPYING', 'COPYRIGHT', 'NOTICE', 'UNLICENSE')


def copy_licenses(component, paths):
    """Copies license files into usr/share/licenses/<component>/."""
    target = os.path.join(LICENSES, component)
    os.makedirs(target, exist_ok=True)
    for path in paths:
        if os.path.isdir(path):
            for name in sorted(os.listdir(path)):
                copy_licenses(component, [os.path.join(path, name)])
        elif os.path.isfile(path):
            shutil.copy(path, os.path.join(target, os.path.basename(path)))


def license_files(folder):
    """The license files at the top of a source folder, and LICENSES/."""
    found = []
    for name in sorted(os.listdir(folder)):
        path = os.path.join(folder, name)
        if name.upper().startswith(LICENSE_NAMES) or name == 'LICENSES':
            found.append(path)
    return found


def unpacked(archive):
    """The source folder of an archive, unpacked once under WORK."""
    with tarfile.open(archive) as tar:
        top = tar.getnames()[0].split('/')[0]
        target = os.path.join(WORK, top)
        if not os.path.isdir(target):
            tar.extractall(WORK, filter='data')
    return target


def elf_files():
    for root, _, files in os.walk(APPDIR):
        for name in files:
            path = os.path.join(root, name)
            if os.path.islink(path) or not os.path.isfile(path):
                continue
            with open(path, 'rb') as file:
                if file.read(4) == b'\x7fELF':
                    yield path


def rpm_owner(name):
    """(package, version, license, source rpm) of the system file `name`."""
    for folder in ('/usr/lib64', '/lib64', '/usr/lib'):
        path = os.path.join(folder, name)
        if os.path.exists(path):
            out = subprocess.run(
                ['rpm', '-qf', '--qf', '%{NAME}\t%{VERSION}-%{RELEASE}\t%{LICENSE}\t%{SOURCERPM}\n',
                 os.path.realpath(path)],
                capture_output=True, text=True, check=False).stdout.strip()
            if out and 'not owned' not in out:
                return tuple(out.splitlines()[0].split('\t'))
    return None


rows = []  # (component, version, license, source)

# orbis itself.
copy_licenses('orbis', ['/src/LICENSE'])
rows.append(('orbis', VERSION, 'GPL-3.0-or-later', 'https://github.com/gddrx/orbis'))

# Qt: its license texts, wherever aqt put them.
qt_texts = [os.path.join(root, name)
            for root, _, files in os.walk(os.path.dirname(QT))
            for name in files
            if 'LICENSE' in root.upper() or name.upper().startswith(('LICENSE', 'LGPL', 'GPL'))]
copy_licenses('qt', qt_texts)
# The texts of Qt's own license, which aqt does not install: the LGPL-3.0
# from a system package that ships it, and the GPL-3.0 it builds on.
for root in ('/usr/share/licenses', '/usr/share/doc'):
    found = subprocess.run(['grep', '-rl', '-s', 'Version 3, 29 June 2007', root],
                           capture_output=True, text=True, check=False).stdout.split()
    lgpl3 = [path for path in found
             if 'GNU LESSER GENERAL PUBLIC LICENSE' in open(path, encoding='utf-8', errors='replace').read(400)]
    if lgpl3:
        os.makedirs(os.path.join(LICENSES, 'qt'), exist_ok=True)
        shutil.copy(lgpl3[0], os.path.join(LICENSES, 'qt', 'LGPL-3.0-only.txt'))
        break
else:
    sys.exit('no LGPL-3.0 text found for Qt')
shutil.copy('/src/LICENSE', os.path.join(LICENSES, 'qt', 'GPL-3.0.txt'))
rows.append(('Qt (with Qt WebEngine and its Chromium)', VERSIONS['QT_VERSION'],
             'LGPL-3.0-only; Qt WebEngine also BSD-3-Clause and the licenses of Chromium',
             'https://download.qt.io/official_releases/qt/'))

# What build-deps.sh built.
built = [
    ('KWindowSystem', 'KWINDOWSYSTEM', 'kwindowsystem', 'LGPL-2.1-or-later',
     f"/sources/kwindowsystem-{VERSIONS['KWINDOWSYSTEM_VERSION']}.tar.xz"),
    ('FFmpeg', 'FFMPEG', 'ffmpeg', 'LGPL-2.1-or-later',
     f"/sources/ffmpeg-{VERSIONS['FFMPEG_VERSION']}.tar.xz"),
    ('mpv (libmpv)', 'MPV', 'mpv', 'LGPL-2.1-or-later',
     f"/sources/v{VERSIONS['MPV_VERSION']}.tar.gz"),
]
for title, key, component, spdx, archive in built:
    copy_licenses(component, license_files(unpacked(archive)))
    rows.append((title, VERSIONS[f'{key}_VERSION'], spdx, VERSIONS[f'{key}_URL']))
placebo = f"/sources/libplacebo-{VERSIONS['LIBPLACEBO_VERSION']}"
copy_licenses('libplacebo', license_files(placebo))
rows.append(('libplacebo', VERSIONS['LIBPLACEBO_VERSION'], 'LGPL-2.1-or-later',
             f"{VERSIONS['LIBPLACEBO_GIT']} (commit {VERSIONS['LIBPLACEBO_COMMIT']})"))

# Node.js and yt-dlp.
copy_licenses('nodejs', license_files(NODE_DIR))
rows.append(('Node.js', VERSIONS['NODE_VERSION'], 'MIT (bundled parts: see its LICENSE)',
             VERSIONS['NODE_URL'].replace('-linux-x64.tar.xz', '.tar.xz')))
yt_dlp = subprocess.run([os.path.join(APPDIR, 'opt/yt-dlp/yt-dlp'), '--version'],
                        capture_output=True, text=True, check=False).stdout.strip()
rows.append(('yt-dlp (standalone build, with Python and its modules)', yt_dlp,
             'Unlicense; the Python inside: PSF-2.0; its modules: see '
             'https://github.com/yt-dlp/yt-dlp/blob/master/THIRD_PARTY_LICENSES.txt',
             f'https://github.com/yt-dlp/yt-dlp/releases/tag/{yt_dlp}'))

# The libraries linuxdeploy took from AlmaLinux.
ours = set()
for root in (QT, '/opt/deps'):
    for folder, _, files in os.walk(root):
        ours.update(files)
packages = {}
for path in elf_files():
    name = os.path.basename(path)
    if not path.startswith(os.path.join(APPDIR, 'usr/lib/')) or name in ours:
        continue
    owner = rpm_owner(name)
    if owner:
        packages.setdefault(owner, []).append(name)
for (package, version, spdx, srpm), names in sorted(packages.items()):
    copy_licenses(f'almalinux/{package}', [f'/usr/share/licenses/{package}'])
    rows.append((f"{package} ({', '.join(sorted(names))})", version, spdx,
                 f'AlmaLinux 9 source package {srpm} (https://vault.almalinux.org/)'))

# The Rust crates in orbis and orbis-login.
metadata = json.loads(subprocess.run(
    ['cargo', 'metadata', '--offline', '--format-version', '1',
     '--config', "source.crates-io.replace-with='vendored'",
     '--config', "source.vendored.directory='/src/target/release-build/vendor'"],
    capture_output=True, text=True, check=True, cwd='/src').stdout)
crates = []
for package in metadata['packages']:
    if package['source'] is None:
        continue
    folder = os.path.dirname(package['manifest_path'])
    copy_licenses(f"rust/{package['name']}-{package['version']}", license_files(folder))
    crates.append((package['name'], package['version'], package.get('license') or '(see its files)',
                   package.get('repository') or f"https://crates.io/crates/{package['name']}"))

os.makedirs(DOC, exist_ok=True)
# Readable by everyone, whatever the originals had.
for root, dirs, files in os.walk(LICENSES):
    for name in dirs:
        os.chmod(os.path.join(root, name), 0o755)
    for name in files:
        os.chmod(os.path.join(root, name), 0o644)
lines = [
    f'# What is inside orbis {VERSION} (x86_64 AppImage)', '',
    'orbis is free software under the GNU General Public License, version 3 or',
    'later. The AppImage also carries the programs and libraries below, each',
    'under its own license; their texts are in `usr/share/licenses/` inside it',
    '(`./orbis-*.AppImage --appimage-extract` unpacks it). The sources of the',
    'LGPL and GPL parts are at the addresses given, and the release on GitHub',
    'carries them as well.', '',
    '| Component | Version | License | Source |', '|---|---|---|---|',
]
lines += [f'| {a} | {b} | {c} | {d} |' for a, b, c, d in rows]
lines += ['', '## Rust crates built into orbis and orbis-login', '',
          '| Crate | Version | License | Source |', '|---|---|---|---|']
lines += [f'| {a} | {b} | {c} | {d} |' for a, b, c, d in sorted(crates)]
text = '\n'.join(lines) + '\n'
with open(os.path.join(DOC, 'THIRD-PARTY.md'), 'w', encoding='utf-8') as file:
    file.write(text)
with open(f'/src/target/appimage/orbis-{VERSION}-THIRD-PARTY.md', 'w', encoding='utf-8') as file:
    file.write(text)
print(f'licenses: {len(rows)} components, {len(packages)} AlmaLinux packages, {len(crates)} crates')
