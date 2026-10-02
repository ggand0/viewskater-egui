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
    // Refuse before the crate gets involved when the folder cannot be
    // written. Moving a file out of a folder needs write permission on
    // the folder, and the freedesktop implementation in trash 5.2.8
    // creates a zero-byte placeholder in the trash before it tries the
    // move and does not remove it when the move fails, so every failed
    // attempt would leave a ghost entry in the user's trash.
    if !parent_is_writable(path) {
        return Err("the folder is read-only".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        // NSFileManager: no Finder Automation permission prompt, no Finder
        // dependency, and it fails with an error on volumes without a
        // trash instead of deleting.
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path).map_err(describe)
    }
    #[cfg(not(target_os = "macos"))]
    {
        trash::delete(path).map_err(describe)
    }
}

/// A short message for the toast instead of the crate's Debug dump.
fn describe(err: trash::Error) -> String {
    match err {
        #[cfg(all(unix, not(target_os = "macos")))]
        trash::Error::FileSystem { source, .. } => match source.kind() {
            std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
            std::io::ErrorKind::NotFound => "file not found".to_string(),
            _ => source.to_string(),
        },
        trash::Error::Os { description, .. } | trash::Error::Unknown { description } => description,
        trash::Error::CouldNotAccess { target } => format!("could not access {target}"),
        trash::Error::TargetedRoot => "cannot trash the root directory".to_string(),
        other => format!("{other:?}"),
    }
}

/// Whether the directory holding `path` allows removing entries. Unix
/// only; Windows permissions are checked by the shell operation itself.
fn parent_is_writable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let Some(parent) = path.parent() else {
            return true;
        };
        let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
        let Ok(c) = CString::new(parent.as_os_str().as_bytes()) else {
            return true;
        };
        // SAFETY: `c` is a valid null-terminated string for the call.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
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

    #[cfg(unix)]
    #[test]
    fn read_only_folder_is_refused_before_the_crate_runs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("locked.txt");
        std::fs::write(&file, b"keep me").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let result = super::move_to_trash(&file);

        // Unlock before asserting so the tempdir can clean itself up.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(result, Err("the folder is read-only".to_string()));
        assert!(file.exists(), "file must stay where it is");
    }

    #[cfg(unix)]
    #[test]
    fn writable_folder_passes_the_precheck() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("free.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(super::parent_is_writable(&file));
        assert!(super::parent_is_writable(Path::new("relative.txt")));
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
