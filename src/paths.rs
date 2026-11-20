//! Every folder the app writes to outside the user's own panes, in one place.
//!
//! | What | Windows | macOS | Linux |
//! |---|---|---|---|
//! | [`data_dir`]: `device_links.tsv`, `fingerprints.tsv` | `%APPDATA%\KitPool` | `~/Library/Application Support/KitPool` | `~/.local/share/kitpool` |
//! | [`cache_dir`]: preview cache (`pool/`, `pool/edits/`) | `%LOCALAPPDATA%\KitPool\cache` | `~/Library/Caches/KitPool` | `~/.cache/kitpool` |
//! | [`log_file`] | `%LOCALAPPDATA%\KitPool\KitPool.log` | `~/Library/Logs/KitPool.log` | `~/.local/state/kitpool.log` |
//! | [`documents`]: projects, patterns, presets, debug dumps | `Documents\KitPool\<sub>` | `~/Documents/KitPool/<sub>` | same |
//! | [`temp`]: staged uploads, device file copies | system temp dir, `kitpool_<name>` | same | same |
//!
//! Window and UI state is stored by eframe under its app id [`APP_ID`]
//! (see `main.rs`), in its own per-platform folder.

use std::path::PathBuf;

/// The app's name for folders and for eframe's storage.
pub const APP_ID: &str = "KitPool";

fn home() -> Option<PathBuf> {
    // HOME is usually unset on Windows outside Git Bash.
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The app's own data folder (links, fingerprints): one per user, not per
/// drive, so files on any volume are known wherever they're used from.
pub fn data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join(APP_ID))
    } else if cfg!(target_os = "macos") {
        Some(home()?.join("Library/Application Support").join(APP_ID))
    } else {
        Some(home()?.join(".local/share/kitpool"))
    }
}

/// Throwaway-but-kept data (the preview cache).
pub fn cache_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(APP_ID).join("cache"))
    } else if cfg!(target_os = "macos") {
        Some(home()?.join("Library/Caches").join(APP_ID))
    } else {
        Some(home()?.join(".cache/kitpool"))
    }
}

/// The diagnostic log; its parent folder is created.
pub fn log_file() -> Option<PathBuf> {
    let p = if cfg!(target_os = "windows") {
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(APP_ID).join("KitPool.log")
    } else if cfg!(target_os = "macos") {
        home()?.join("Library/Logs/KitPool.log")
    } else {
        home()?.join(".local/state/kitpool.log")
    };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    Some(p)
}

/// `~/Documents/KitPool/<sub>`: default place for files the user saves
/// (projects, patterns, presets) and debug dumps. Not created here.
pub fn documents(sub: &str) -> Option<PathBuf> {
    Some(home()?.join("Documents").join(APP_ID).join(sub))
}

/// `<system temp>/kitpool_<name>`. Not created here.
pub fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("kitpool_{name}"))
}
