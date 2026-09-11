//! Moving files to the platform trash.
//!
//! This is the only module in the app that takes a file out of its
//! directory, and it never unlinks. Every path goes through the `trash`
//! crate, which moves it to the Recycle Bin, the macOS Trash, or a
//! freedesktop trash folder. When the platform has no trash for a location
//! the crate returns an error and the file stays where it is, except on
//! Windows where the shell would delete permanently; `lacks_recycle_bin`
//! lets the caller ask the user first in that case.

use std::path::Path;

/// Move `path` to the platform trash. Returns a message for the user on
/// failure. The file is untouched when this returns `Err`.
pub(crate) fn move_to_trash(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // NSFileManager: no Finder Automation permission prompt, no Finder
        // dependency, and it fails with an error on volumes without a
        // trash instead of deleting.
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        trash::delete(path).map_err(|e| e.to_string())
    }
}

/// True when the platform would delete `path` permanently instead of
/// recycling it. Only Windows does that: network shares and removable
/// media have no Recycle Bin, and the shell deletes from them for real.
/// Linux and macOS return an error from `move_to_trash` instead, so this
/// is always false there.
pub(crate) fn lacks_recycle_bin(path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::lacks_recycle_bin(path)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        false
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Path, Prefix};

    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows_sys::Win32::System::WindowsProgramming::{DRIVE_REMOTE, DRIVE_REMOVABLE};

    pub(super) fn lacks_recycle_bin(path: &Path) -> bool {
        let Ok(path) = std::path::absolute(path) else {
            return false;
        };
        let Some(Component::Prefix(prefix)) = path.components().next() else {
            return false;
        };
        match prefix.kind() {
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) => true,
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                let root = format!("{}:\\", letter as char);
                drive_type_lacks_bin(&root)
            }
            _ => false,
        }
    }

    fn drive_type_lacks_bin(root: &str) -> bool {
        let wide: Vec<u16> = std::ffi::OsStr::new(root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is a valid null-terminated UTF-16 string that
        // outlives the call.
        let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
        kind == DRIVE_REMOTE || kind == DRIVE_REMOVABLE
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The app must never unlink a file. This scans every source file for
    /// the std deletion calls so a future refactor cannot sneak one in.
    /// The needles are assembled at runtime so this file does not match
    /// itself.
    #[test]
    fn no_permanent_deletion_calls_in_source() {
        let needles: Vec<String> = ["remove_", "remove_", "remove_"]
            .iter()
            .zip(["file(", "dir(", "dir_all("])
            .map(|(a, b)| format!("{a}{b}"))
            .collect();

        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);
        assert!(files.len() > 5, "expected to find the source tree");

        let mut hits = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).unwrap();
            for (line_no, line) in text.lines().enumerate() {
                if needles.iter().any(|n| line.contains(n.as_str())) {
                    hits.push(format!("{}:{}: {}", file.display(), line_no + 1, line.trim()));
                }
            }
        }
        assert!(
            hits.is_empty(),
            "permanent deletion calls found in source:\n{}",
            hits.join("\n")
        );
    }

    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Round trip through the real trash. Ignored because it touches the
    /// user's trash folder; run by hand with `cargo test -- --ignored`.
    /// The item is purged afterwards on platforms where the crate can list
    /// the trash (Linux and Windows).
    #[test]
    #[ignore]
    fn real_trash_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("viewskater_trash_test.txt");
        std::fs::write(&file, b"trash me").unwrap();

        super::move_to_trash(&file).expect("move_to_trash failed");
        assert!(!file.exists(), "file should be gone from its directory");

        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            let items = trash::os_limited::list().unwrap();
            let name = file.file_name().unwrap();
            let ours: Vec<_> = items
                .into_iter()
                .filter(|item| item.name == *name)
                .collect();
            assert!(!ours.is_empty(), "item should be listed in the trash");
            trash::os_limited::purge_all(ours).unwrap();
        }
    }
}
