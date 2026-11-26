//! Pure cross-platform application directory resolution (`slackless-core`).
//!
//! Resolves standard OS data, cache, and configuration paths without depending on
//! `slackless-platform` (or linking AppKit / GUI frameworks).

use std::path::PathBuf;

/// Application data directory (`~/Library/Application Support/slackless` on macOS,
/// `$XDG_DATA_HOME/slackless` or `~/.local/share/slackless` on Linux,
/// `%APPDATA%\slackless` on Windows).
pub fn app_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("slackless"))
}

/// Application cache directory (`~/Library/Caches/slackless` on macOS,
/// `$XDG_CACHE_HOME/slackless` or `~/.cache/slackless` on Linux,
/// `%LOCALAPPDATA%\slackless` on Windows).
pub fn app_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("slackless"))
}

/// Application configuration directory (`~/Library/Application Support/slackless` on macOS,
/// `$XDG_CONFIG_HOME/slackless` or `~/.config/slackless` on Linux,
/// `%APPDATA%\slackless` on Windows).
pub fn app_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("slackless"))
}

/// Durable SQLite archive path for one workspace (`<app_data_dir>/<team_id>.sqlite`).
pub fn archive_store_path(team_id: impl AsRef<str>) -> Option<PathBuf> {
    app_data_dir().map(|dir| dir.join(format!("{}.sqlite", team_id.as_ref())))
}

/// Filename stem reserved for the sidecar semantic vector index (`semantic_index.sqlite`).
pub const SEMANTIC_INDEX_STEM: &str = "semantic_index";

/// Dedicated sidecar SQLite vector index path (`<app_data_dir>/semantic_index.sqlite`), stored
/// next to workspace archive databases without modifying their schema.
pub fn semantic_index_path() -> Option<PathBuf> {
    app_data_dir().map(|dir| dir.join(format!("{SEMANTIC_INDEX_STEM}.sqlite")))
}

/// Directory storing opt-in local ONNX sentence embedding models
/// (`<app_data_dir>/models/embeddings`).
pub fn semantic_models_dir() -> Option<PathBuf> {
    app_data_dir().map(|dir| dir.join("models").join("embeddings"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_directories_and_archive_store_path_are_consistent() {
        let data = app_data_dir().expect("data dir");
        let cache = app_cache_dir().expect("cache dir");
        let config = app_config_dir().expect("config dir");

        assert_eq!(data.file_name().and_then(|s| s.to_str()), Some("slackless"));
        assert_eq!(
            cache.file_name().and_then(|s| s.to_str()),
            Some("slackless")
        );
        assert_eq!(
            config.file_name().and_then(|s| s.to_str()),
            Some("slackless")
        );

        #[cfg(target_os = "macos")]
        {
            assert!(
                data.ends_with("Library/Application Support/slackless"),
                "unexpected macOS data dir: {data:?}"
            );
            assert!(
                config.ends_with("Library/Application Support/slackless"),
                "unexpected macOS config dir: {config:?}"
            );
            assert!(
                cache.ends_with("Library/Caches/slackless"),
                "unexpected macOS cache dir: {cache:?}"
            );
        }

        let archive = archive_store_path("T0123ABC").expect("archive path");
        assert_eq!(archive, data.join("T0123ABC.sqlite"));

        let team_id = crate::types::TeamId::from("T9999XYZ");
        assert_eq!(
            archive_store_path(&team_id).expect("archive path for TeamId"),
            data.join("T9999XYZ.sqlite")
        );
    }
}
