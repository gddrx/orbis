//! The user's folders as the desktop names them (XDG user directories):
//! `$XDG_CONFIG_HOME/user-dirs.dirs`, written by `xdg-user-dirs-update`;
//! and orbis's own cache folder.

use std::path::{Path, PathBuf};

/// The value of `key` in the text of a `user-dirs.dirs` file: a quoted
/// path that is absolute or starts with `$HOME`.
fn parse(text: &str, key: &str, home: &Path) -> Option<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let value = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
            let value = value.trim().trim_matches('"');
            if let Some(rest) = value.strip_prefix("$HOME") {
                let rest = rest.trim_start_matches('/');
                Some(if rest.is_empty() {
                    home.to_owned()
                } else {
                    home.join(rest)
                })
            } else if value.starts_with('/') {
                Some(PathBuf::from(value))
            } else {
                None
            }
        })
}

/// The music folder: `$XDG_MUSIC_DIR`, else the desktop's setting in
/// `user-dirs.dirs`, else `~/Music`.
pub fn music_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_MUSIC_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|text| parse(&text, "XDG_MUSIC_DIR", &home))
        // A folder set to the home itself means "none" to the desktop.
        .filter(|dir| dir != &home)
        .unwrap_or_else(|| home.join("Music"))
}

/// orbis's cache folder: `$ORBIS_CACHE_DIR`, else `$XDG_CACHE_HOME/orbis`,
/// else `~/.cache/orbis`.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ORBIS_CACHE_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("orbis")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_dirs_are_read_as_the_desktop_writes_them() {
        let home = Path::new("/home/u");
        let text = "# comment\nXDG_DESKTOP_DIR=\"$HOME/desktop\"\nXDG_MUSIC_DIR=\"$HOME/music\"\nXDG_VIDEOS_DIR=\"$HOME/\"\nXDG_PICTURES_DIR=\"/data/pictures\"\n";
        assert_eq!(
            parse(text, "XDG_MUSIC_DIR", home),
            Some(PathBuf::from("/home/u/music"))
        );
        assert_eq!(
            parse(text, "XDG_VIDEOS_DIR", home),
            Some(PathBuf::from("/home/u"))
        );
        assert_eq!(
            parse(text, "XDG_PICTURES_DIR", home),
            Some(PathBuf::from("/data/pictures"))
        );
        assert_eq!(parse(text, "XDG_TEMPLATES_DIR", home), None);
        assert_eq!(
            parse("XDG_MUSIC_DIR=\"music\"", "XDG_MUSIC_DIR", home),
            None
        );
    }
}
