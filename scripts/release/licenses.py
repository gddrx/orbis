"""Writes the licenses of everything in the release AppDir (runs inside the
release container, from build-appimage.sh):

- usr/share/licenses/<component>/ — the license texts;
- usr/share/doc/orbis/THIRD-PARTY.md — what is inside, its version, its
  license and where its source is; a copy goes next to the AppImage.

Where a library came from is found by its file name: Qt's own folder, the
libraries build-deps.sh made in /opt/deps, else the AlmaLinux package that
owns it (rpm -qf), whose license files are copied — from the package, its
documentation, the packages built from the same source, or
scripts/release/licenses/. The Rust crates are listed from cargo metadata,
with the license files of their vendored sources; a crate that ships none
gets the text its license names. Anything left without a license text
stops the build.

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
missing = []  # what no license text was found for
EXTRA = '/scripts/licenses'


def rpm_query(*args):
    return subprocess.run(['rpm', *args], capture_output=True, text=True,
                          check=False).stdout.split()


def alma_texts(package, srpm):
    """License files of an AlmaLinux package: its own, its documentation's
    COPYING or LICENSE, those of the packages built from the same source,
    or what scripts/release/licenses/ keeps for it."""
    siblings = [package] + [line.split()[0] for line in subprocess.run(
        ['rpm', '-qa', '--qf', '%{NAME} %{SOURCERPM}\n'], capture_output=True, text=True,
        check=False).stdout.splitlines() if line.endswith(' ' + srpm) and not line.startswith(package + ' ')]
    for name in siblings:
        found = [path for path in rpm_query('-qL', name) if os.path.isfile(path)]
        found += [path for path in rpm_query('-qd', name)
                  if os.path.isfile(path) and os.path.basename(path).upper().startswith(LICENSE_NAMES)]
        if found:
            return found
    extra = os.path.join(EXTRA, package)
    return [extra] if os.path.isdir(extra) else []


def crate_texts(package, target):
    """The text a crate without license files is under: Apache-2.0 when its
    license offers it, else MIT or BSD-3-Clause with its authors' notice."""
    spdx = package.get('license') or ''
    authors = ', '.join(package.get('authors') or []) or f"the {package['name']} authors"
    os.makedirs(target, exist_ok=True)
    if 'Apache-2.0' in spdx:
        shutil.copy(os.path.join(EXTRA, 'Apache-2.0.txt'), os.path.join(target, 'LICENSE-Apache-2.0'))
        if spdx != 'Apache-2.0':
            with open(os.path.join(target, 'CHOICE'), 'w', encoding='utf-8') as file:
                file.write(f'{spdx}: taken under Apache-2.0.\n')
        return True
    if spdx == 'MIT':
        text = MIT
    elif spdx == 'BSD-3-Clause':
        text = BSD3
    else:
        return False
    with open(os.path.join(target, f'LICENSE-{spdx}'), 'w', encoding='utf-8') as file:
        file.write(f'Copyright (c) {authors}\n\n{text}')
    return True


MIT = """Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""

BSD3 = """Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
"""

# orbis itself.
copy_licenses('orbis', ['/src/LICENSE'])
rows.append(('orbis', VERSION, 'GPL-3.0-or-later', 'https://github.com/gddrx/orbis'))
copy_licenses('material-icons', ['/src/crates/app/src/qml/icons/LICENSE-Apache-2.0.txt'])
rows.append(('Material Icons by Google (most icons of the window)', '-', 'Apache-2.0',
             'https://github.com/google/material-design-icons'))

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
if not os.path.isfile('/tools/yt-dlp-THIRD_PARTY_LICENSES.txt'):
    missing.append('yt-dlp: /tools/yt-dlp-THIRD_PARTY_LICENSES.txt (appimage-tools.sh fetches it)')
copy_licenses('yt-dlp', ['/tools/yt-dlp-THIRD_PARTY_LICENSES.txt'])
rows.append(('yt-dlp (standalone build, with Python and its modules)', yt_dlp,
             'Unlicense; the Python inside: PSF-2.0; its modules: their own '
             '(usr/share/licenses/yt-dlp/)',
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
    texts = alma_texts(package, srpm)
    if not texts:
        missing.append(f'AlmaLinux package {package}')
    copy_licenses(f'almalinux/{package}', texts)
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
    component = f"rust/{package['name']}-{package['version']}"
    files = license_files(folder)
    if files:
        copy_licenses(component, files)
    elif not crate_texts(package, os.path.join(LICENSES, component)):
        missing.append(f"crate {package['name']} {package['version']} ({package.get('license')})")
    crates.append((package['name'], package['version'], package.get('license') or '(see its files)',
                   package.get('repository') or f"https://crates.io/crates/{package['name']}"))

if missing:
    sys.exit('no license text for:\n  ' + '\n  '.join(missing))
empty = [os.path.relpath(root, LICENSES) for root, dirs, files in os.walk(LICENSES)
         if not dirs and not files]
if empty:
    sys.exit('empty license folders:\n  ' + '\n  '.join(empty))
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
