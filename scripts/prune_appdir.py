"""Drops from the AppDir what orbis never loads, once linuxdeploy has filled
it (see appimage.sh):

- the Qt Quick Controls styles other than Basic, orbis's own, and Fusion,
  Qt's default on Linux, which the folder dialog of QtQuick.Dialogs takes
  when no desktop dialog is at hand; with the files for Qt Design Studio;
- the image format plugins other than JPEG, WebP and SVG (covers and
  icons; PNG is built into Qt);
- then the libraries only those needed;
- and the copies of libraries under their full version (libssl.so.3.5.3
  next to libssl.so.3): everything asks for the short name, its SONAME.

The libraries nothing needs before the pruning are kept but those copies:
a program may open them itself. Prints what went and how much.

Usage: python3 -I prune_appdir.py <AppDir>
"""

import os
import shutil
import subprocess
import sys

STYLES = ('Material', 'Universal', 'Imagine', 'FluentWinUI3', 'designer')
IMAGE_FORMATS = {'libqjpeg.so', 'libqwebp.so', 'libqsvg.so'}


def elf_files(appdir):
    """The ELF files of the AppDir, as paths relative to it."""
    for root, _, files in os.walk(appdir):
        for name in files:
            path = os.path.join(root, name)
            if os.path.islink(path):
                continue
            with open(path, 'rb') as file:
                if file.read(4) == b'\x7fELF':
                    yield os.path.relpath(path, appdir)


def dynamic(appdir, path):
    """The NEEDED names and the SONAME of an ELF file."""
    out = subprocess.run(['readelf', '-d', os.path.join(appdir, path)],
                         capture_output=True, text=True, check=False).stdout
    needed, soname = set(), None
    for line in out.splitlines():
        if '[' not in line:
            continue
        value = line.split('[', 1)[1].split(']', 1)[0]
        if '(NEEDED)' in line:
            needed.add(value)
        elif '(SONAME)' in line:
            soname = value
    return needed, soname


def needed_libraries(appdir, files, info):
    """The names of usr/lib files that the files outside usr/lib need, at
    any depth."""
    libraries = {os.path.basename(f): f for f in files if f.startswith('usr/lib/')}
    seen = set()
    todo = [f for f in files if not f.startswith('usr/lib/')]
    while todo:
        for name in info[todo.pop()][0]:
            if name in libraries and name not in seen:
                seen.add(name)
                todo.append(libraries[name])
    return seen


def size(appdir, path):
    full = os.path.join(appdir, path)
    if os.path.isdir(full):
        return sum(os.path.getsize(os.path.join(root, f))
                   for root, _, names in os.walk(full) for f in names)
    return os.path.getsize(full)


def main():
    appdir = sys.argv[1]
    files = list(elf_files(appdir))
    info = {f: dynamic(appdir, f) for f in files}
    libraries = {f for f in files if f.startswith('usr/lib/')}
    needed_before = needed_libraries(appdir, files, info)

    dropped = []
    controls = 'usr/qml/QtQuick/Controls'
    dropped += [f'{controls}/{style}' for style in STYLES
                if os.path.isdir(os.path.join(appdir, controls, style))]
    formats = 'usr/plugins/imageformats'
    if os.path.isdir(os.path.join(appdir, formats)):
        dropped += [f'{formats}/{name}' for name in sorted(os.listdir(os.path.join(appdir, formats)))
                    if name not in IMAGE_FORMATS]

    def gone(path):
        return any(path == d or path.startswith(d + '/') for d in dropped)

    left = [f for f in files if not gone(f)]
    needed_after = needed_libraries(appdir, left, info)
    dropped += sorted(f for f in libraries
                      if os.path.basename(f) in needed_before - needed_after)

    # Copies under the full version: no file asks for that name.
    asked = set().union(*(info[f][0] for f in left))
    dropped += sorted(f for f in libraries
                      if info[f][1] and info[f][1] != os.path.basename(f)
                      and os.path.basename(f) not in asked and f not in dropped)

    total = 0
    for path in dropped:
        bytes_ = size(appdir, path)
        total += bytes_
        print(f'  {bytes_ / 1e6:5.1f} MB  {path}')
        full = os.path.join(appdir, path)
        if os.path.isdir(full):
            shutil.rmtree(full)
        else:
            os.remove(full)
    print(f'pruned {len(dropped)} files and folders, {total / 1e6:.1f} MB')


if __name__ == '__main__':
    main()
