//! The recently opened files and folders behind File > Open Recent.
//!
//! The list is newest first, holds at most `MAX_ENTRIES` paths and never
//! the same path twice. It is kept in recent.yaml next to settings.yaml and
//! written after every change. An entry is the path as the user opened it,
//! the file for a file open and the folder for a folder open.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_ENTRIES: usize = 10;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RecentPaths {
    paths: Vec<PathBuf>,
}

impl RecentPaths {
    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("viewskater-egui").join("recent.yaml"))
    }

    /// Read the list from the default file. A missing or unreadable file is
    /// an empty list.
    pub(crate) fn load() -> Self {
        Self::config_path()
            .map(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    /// Read the list from `path`. A missing or unreadable file is an empty
    /// list.
    pub(crate) fn load_from(path: &Path) -> Self {
        let mut recent: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_yaml::from_str(&s).ok())
            .unwrap_or_default();
        recent.paths.truncate(MAX_ENTRIES);
        log::debug!("Loaded {} recent paths from {}", recent.paths.len(), path.display());
        recent
    }

    pub(crate) fn save(&self) {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match serde_yaml::to_string(self) {
                Ok(yaml) => {
                    if let Err(e) = std::fs::write(&path, yaml) {
                        log::error!("Failed to save recent paths to {}: {}", path.display(), e);
                    }
                }
                Err(e) => log::error!("Failed to serialize recent paths: {}", e),
            }
        }
    }

    /// Put `path` at the front. An equal entry further down is removed, and
    /// the oldest entry drops off past `MAX_ENTRIES`.
    pub(crate) fn push(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(MAX_ENTRIES);
    }

    pub(crate) fn remove(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
    }

    pub(crate) fn clear(&mut self) {
        self.paths.clear();
    }

    pub(crate) fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(paths: &[&str]) -> RecentPaths {
        let mut recent = RecentPaths::default();
        for p in paths.iter().rev() {
            recent.push(Path::new(p));
        }
        recent
    }

    #[test]
    fn push_puts_new_path_first() {
        let recent = list(&["/b", "/a"]);
        assert_eq!(recent.paths(), [PathBuf::from("/b"), PathBuf::from("/a")]);
    }

    #[test]
    fn push_moves_existing_path_to_front_without_duplicate() {
        let mut recent = list(&["/c", "/b", "/a"]);
        recent.push(Path::new("/a"));
        assert_eq!(
            recent.paths(),
            [PathBuf::from("/a"), PathBuf::from("/c"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn list_stops_at_max_entries() {
        let mut recent = RecentPaths::default();
        for i in 0..MAX_ENTRIES + 3 {
            recent.push(&PathBuf::from(format!("/dir{i}")));
        }
        assert_eq!(recent.paths().len(), MAX_ENTRIES);
        assert_eq!(recent.paths()[0], PathBuf::from(format!("/dir{}", MAX_ENTRIES + 2)));
        assert_eq!(recent.paths()[MAX_ENTRIES - 1], PathBuf::from("/dir3"));
    }

    #[test]
    fn remove_drops_only_that_path() {
        let mut recent = list(&["/c", "/b", "/a"]);
        recent.remove(Path::new("/b"));
        assert_eq!(recent.paths(), [PathBuf::from("/c"), PathBuf::from("/a")]);
        recent.remove(Path::new("/missing"));
        assert_eq!(recent.paths().len(), 2);
    }

    #[test]
    fn clear_empties_list() {
        let mut recent = list(&["/b", "/a"]);
        recent.clear();
        assert!(recent.paths().is_empty());
    }

    #[test]
    fn yaml_round_trip() {
        let recent = list(&["/photos/a.jpg", "/photos/2026 trip"]);
        let yaml = serde_yaml::to_string(&recent).unwrap();
        assert_eq!(yaml, "- /photos/a.jpg\n- /photos/2026 trip\n");
        let back: RecentPaths = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, recent);
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let recent = RecentPaths::load_from(&dir.path().join("recent.yaml"));
        assert!(recent.paths().is_empty());
    }

    #[test]
    fn unreadable_file_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent.yaml");
        std::fs::write(&path, "not: [a list").unwrap();
        assert!(RecentPaths::load_from(&path).paths().is_empty());
    }
}
