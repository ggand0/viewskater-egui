//! Keeps the stars on images. Each folder with a starred image gets a
//! hidden `.viewskater.yaml` that lists them by file name, so the stars
//! move with the folder.
//!
//! Every entry keeps the file's size from when it was starred. An entry
//! whose file is gone or has another size shows no star but stays in the
//! file: a folder listing that skipped a few entries, a NAS dropping them,
//! must not cost those photos their stars. An entry leaves the file only
//! when its image is unstarred or moved to the trash.
//!
//! Saves run in order on one writer thread, so a slow share never blocks
//! the UI. Each save reads the file again, changes one entry and writes
//! the result to a hidden temp file that is renamed over the old one.
//!
//! The app also keeps a list of the folders that have the file, in
//! `star_folders.yaml` next to settings.yaml. Nothing else could find
//! them again: the Stars tab in Preferences shows the list and moves the
//! files to the trash for someone who wants them gone.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use chrono::{Local, SecondsFormat};
use eframe::egui;
use serde::{Deserialize, Serialize};

/// The file in each folder that has starred images.
pub(crate) const FILE_NAME: &str = ".viewskater.yaml";
/// A save writes here first, then renames it over `FILE_NAME`.
const TEMP_NAME: &str = ".viewskater.yaml.tmp";
/// The format this build reads and writes. A file with a higher version
/// comes from a newer build and is never written over.
const VERSION: u32 = 1;

/// Where the list of folders with a `.viewskater.yaml` is kept.
pub(crate) fn folder_list_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("viewskater-egui").join("star_folders.yaml"))
}

/// The contents of `star_folders.yaml`.
#[derive(Serialize, Deserialize)]
struct FolderList {
    version: u32,
    #[serde(default)]
    folders: BTreeSet<PathBuf>,
}

/// The contents of one folder's `.viewskater.yaml`.
#[derive(Serialize, Deserialize)]
struct StarFile {
    version: u32,
    #[serde(default)]
    stars: BTreeMap<String, StarEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StarEntry {
    /// Always 1 for now. A number, so grades and `xmp:Rating` fit later.
    rating: u8,
    /// Size of the file in bytes when it was starred.
    size: u64,
    /// When the star was set, RFC 3339 in local time.
    changed: String,
}

/// One folder's stars as the app shows them.
#[derive(Default)]
struct FolderStars {
    /// Names whose entry matches the file on disk. These show a star.
    starred: HashSet<String>,
    /// Every name in the folder's file, starred or not.
    names_in_file: HashSet<String>,
    /// Why the folder's file must not be written: it could not be read,
    /// or a newer build saved it.
    locked: Option<String>,
    /// Saves sent to the writer that have not finished.
    pending_writes: usize,
}

enum Change {
    Star(StarEntry),
    Unstar,
}

struct WriteJob {
    dir: PathBuf,
    name: String,
    change: Change,
    /// How the name stood before this change, to put back if the save
    /// fails.
    was_starred: bool,
    was_in_file: bool,
}

struct FinishedWrite {
    dir: PathBuf,
    name: String,
    starred: bool,
    was_starred: bool,
    was_in_file: bool,
    result: Result<(), String>,
}

/// What moving every star file to the trash did.
#[derive(Default)]
pub(crate) struct StarFileMoves {
    /// Files now in the trash.
    pub moved: usize,
    /// Files on a Windows network share or removable drive, where the
    /// shell would delete them for good instead. They stay where they are.
    pub left_in_place: Vec<PathBuf>,
    /// Files the trash refused, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

/// The stars of every folder the panes list, shared by both panes.
pub(crate) struct Stars {
    folders: HashMap<PathBuf, FolderStars>,
    /// `star_folders.yaml`, or `None` to keep the list in memory only.
    folder_list: Option<PathBuf>,
    /// Absolute paths of the folders known to have a `.viewskater.yaml`.
    star_file_folders: BTreeSet<PathBuf>,
    jobs: Option<Sender<WriteJob>>,
    finished: Receiver<FinishedWrite>,
    writer: Option<JoinHandle<()>>,
}

impl Stars {
    /// `folder_list` is `star_folders.yaml` (`folder_list_path`), or `None`
    /// to keep the list of folders in memory only.
    pub(crate) fn new(ctx: &egui::Context, folder_list: Option<PathBuf>) -> Self {
        let (jobs, job_receiver) = mpsc::channel::<WriteJob>();
        let (finished_sender, finished) = mpsc::channel();
        let ctx = ctx.clone();
        let writer = std::thread::Builder::new()
            .name("stars-writer".into())
            .spawn(move || {
                for job in job_receiver {
                    let result = save_change(&job.dir, &job.name, &job.change);
                    if let Err(e) = &result {
                        log::error!("Could not save the star of {} in {}: {e}", job.name, job.dir.display());
                    }
                    let failed = result.is_err();
                    let _ = finished_sender.send(FinishedWrite {
                        dir: job.dir,
                        name: job.name,
                        starred: matches!(job.change, Change::Star(_)),
                        was_starred: job.was_starred,
                        was_in_file: job.was_in_file,
                        result,
                    });
                    // A failure takes the star back and shows a toast, so
                    // the app should not wait for the next input to draw.
                    if failed {
                        ctx.request_repaint();
                    }
                }
            })
            .expect("spawn the stars writer thread");
        let star_file_folders = match &folder_list {
            Some(path) => read_folder_list(path).unwrap_or_else(|e| {
                log::warn!("{e}");
                BTreeSet::new()
            }),
            None => BTreeSet::new(),
        };
        Self {
            folders: HashMap::new(),
            folder_list,
            star_file_folders,
            jobs: Some(jobs),
            finished,
            writer: Some(writer),
        }
    }

    /// Read the stars of every folder that holds one of `images`, after a
    /// folder listing. `star_files` are the `.viewskater.yaml` files the
    /// listing walked past. A folder without one gets no stars. A folder
    /// with saves still on their way to disk keeps what it has, because
    /// its file does not have those changes yet.
    pub(crate) fn load(&mut self, images: &[PathBuf], star_files: &[PathBuf]) {
        let with_file: HashSet<&Path> = star_files.iter().filter_map(|f| f.parent()).collect();
        // A file from before the list existed, from another computer or in
        // a copied folder joins the list here.
        let unlisted: Vec<PathBuf> = with_file
            .iter()
            .filter_map(|dir| std::path::absolute(dir).ok())
            .filter(|dir| !self.star_file_folders.contains(dir))
            .collect();
        if !unlisted.is_empty() {
            self.update_folder_list(&unlisted, &[]);
        }
        let dirs: HashSet<&Path> = images.iter().filter_map(|p| p.parent()).collect();
        for dir in dirs {
            if self.folders.get(dir).is_some_and(|f| f.pending_writes > 0) {
                continue;
            }
            let folder = if with_file.contains(dir) {
                read_folder(dir)
            } else {
                FolderStars::default()
            };
            self.folders.insert(dir.to_path_buf(), folder);
        }
    }

    pub(crate) fn is_starred(&self, path: &Path) -> bool {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
            return false;
        };
        self.folders.get(dir).is_some_and(|f| f.starred.contains(name))
    }

    /// Star `path`, whose file is `size` bytes now. The star shows at once
    /// and the save follows on the writer thread. Err is the message for
    /// the user.
    pub(crate) fn star(&mut self, path: &Path, size: u64) -> Result<(), String> {
        let entry = StarEntry {
            rating: 1,
            size,
            changed: Local::now().to_rfc3339_opts(SecondsFormat::Secs, false),
        };
        self.change(path, Change::Star(entry))
    }

    pub(crate) fn unstar(&mut self, path: &Path) -> Result<(), String> {
        self.change(path, Change::Unstar)
    }

    /// `path` was moved to the trash. Its entry leaves the folder's file,
    /// so a new file with the same name later does not show a star.
    pub(crate) fn forget(&mut self, path: &Path) {
        let in_file = split(path).ok().is_some_and(|(dir, name)| {
            self.folders.get(dir).is_some_and(|f| f.names_in_file.contains(&name))
        });
        if in_file {
            if let Err(e) = self.change(path, Change::Unstar) {
                log::warn!("{e}");
            }
        }
    }

    fn change(&mut self, path: &Path, change: Change) -> Result<(), String> {
        let starring = matches!(change, Change::Star(_));
        let (dir, name) = split(path).map_err(|reason| failure_message(starring, path, &reason))?;
        if let Some(reason) = self.folders.get(dir).and_then(|f| f.locked.as_ref()) {
            return Err(failure_message(starring, path, reason));
        }
        // Listed before the file exists, so the list never misses one. A
        // save that fails leaves a folder without the file in the list,
        // which the Stars tab passes over.
        if starring {
            if let Ok(absolute) = std::path::absolute(dir) {
                if !self.star_file_folders.contains(&absolute) {
                    self.update_folder_list(&[absolute], &[]);
                }
            }
        }
        let folder = self.folders.entry(dir.to_path_buf()).or_default();
        let was_starred = folder.starred.contains(&name);
        let was_in_file = folder.names_in_file.contains(&name);
        if starring {
            folder.starred.insert(name.clone());
            folder.names_in_file.insert(name.clone());
        } else {
            folder.starred.remove(&name);
            folder.names_in_file.remove(&name);
        }
        folder.pending_writes += 1;
        let job = WriteJob { dir: dir.to_path_buf(), name: name.clone(), change, was_starred, was_in_file };
        let sent = self.jobs.as_ref().is_some_and(|jobs| jobs.send(job).is_ok());
        if !sent {
            let folder = self.folders.entry(dir.to_path_buf()).or_default();
            folder.pending_writes -= 1;
            restore(folder, &name, was_starred, was_in_file);
            return Err(failure_message(starring, path, "the writer thread stopped"));
        }
        Ok(())
    }

    /// Collect the saves the writer finished since the last call. A failed
    /// save takes its change back. Returns one message per failure, for
    /// the user.
    pub(crate) fn take_failures(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        while let Ok(done) = self.finished.try_recv() {
            let folder = self.folders.entry(done.dir.clone()).or_default();
            folder.pending_writes = folder.pending_writes.saturating_sub(1);
            if let Err(reason) = done.result {
                restore(folder, &done.name, done.was_starred, done.was_in_file);
                failures.push(failure_message(done.starred, &done.dir.join(&done.name), &reason));
            }
        }
        failures
    }

    /// Absolute paths of the folders known to have a `.viewskater.yaml`.
    pub(crate) fn star_file_folders(&self) -> &BTreeSet<PathBuf> {
        &self.star_file_folders
    }

    pub(crate) fn has_pending_writes(&self) -> bool {
        self.folders.values().any(|f| f.pending_writes > 0)
    }

    /// Move the `.viewskater.yaml` of every listed folder to the trash with
    /// `move_to_trash`, the way Move to Trash moves an image. The app never
    /// deletes one. `lacks_trash` names the locations where the trash would
    /// delete for good (Windows network shares and removable drives), and
    /// those files stay. Call it with no saves pending, or a save could put
    /// a file back right after it left.
    ///
    /// The folders whose file is gone leave the list and show no stars.
    pub(crate) fn move_star_files(
        &mut self,
        move_to_trash: impl Fn(&Path) -> Result<(), String>,
        lacks_trash: impl Fn(&Path) -> bool,
    ) -> StarFileMoves {
        let mut moves = StarFileMoves::default();
        let mut gone = Vec::new();
        for dir in &self.star_file_folders {
            let file = dir.join(FILE_NAME);
            if !file.is_file() {
                gone.push(dir.clone());
                continue;
            }
            if lacks_trash(&file) {
                moves.left_in_place.push(file);
                continue;
            }
            match move_to_trash(&file) {
                Ok(()) => {
                    moves.moved += 1;
                    gone.push(dir.clone());
                    // A temp file only stays behind after a crash mid-save.
                    let temp = dir.join(TEMP_NAME);
                    if temp.is_file() {
                        let _ = move_to_trash(&temp);
                    }
                }
                Err(e) => moves.failed.push((file, e)),
            }
        }
        for (dir, folder) in &mut self.folders {
            let absolute = std::path::absolute(dir).unwrap_or_else(|_| dir.clone());
            if gone.contains(&absolute) {
                *folder = FolderStars::default();
            }
        }
        self.update_folder_list(&[], &gone);
        moves
    }

    /// Add and remove folders in the list, on disk too. The file is read
    /// again first, so folders another window added stay in it.
    fn update_folder_list(&mut self, add: &[PathBuf], remove: &[PathBuf]) {
        let mut folders = match &self.folder_list {
            Some(path) => match read_folder_list(path) {
                Ok(folders) => folders,
                Err(e) => {
                    // Never written over, like a star file this build
                    // cannot read. The list lives on in memory.
                    log::warn!("{e}");
                    self.star_file_folders.extend(add.iter().cloned());
                    self.star_file_folders.retain(|f| !remove.contains(f));
                    return;
                }
            },
            None => self.star_file_folders.clone(),
        };
        folders.extend(add.iter().cloned());
        folders.retain(|f| !remove.contains(f));
        if let Some(path) = &self.folder_list {
            if let Err(e) = write_folder_list(path, &folders) {
                log::error!("Could not save {}: {e}", path.display());
            }
        }
        self.star_file_folders = folders;
    }

    /// Let the writer finish every save in its queue, then stop it. Runs
    /// when the app closes, so a star set just before quitting is saved.
    pub(crate) fn finish_writes(&mut self) {
        self.jobs = None;
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }

    /// Wait until the writer has finished every save sent so far.
    #[cfg(test)]
    pub(crate) fn wait_for_writes(&mut self) -> Vec<String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut failures = Vec::new();
        loop {
            failures.extend(self.take_failures());
            let pending = self.folders.values().any(|f| f.pending_writes > 0);
            if !pending || std::time::Instant::now() > deadline {
                return failures;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

impl Drop for Stars {
    fn drop(&mut self) {
        self.finish_writes();
    }
}

fn restore(folder: &mut FolderStars, name: &str, was_starred: bool, was_in_file: bool) {
    if was_starred {
        folder.starred.insert(name.to_string());
    } else {
        folder.starred.remove(name);
    }
    if was_in_file {
        folder.names_in_file.insert(name.to_string());
    } else {
        folder.names_in_file.remove(name);
    }
}

fn failure_message(starring: bool, path: &Path, reason: &str) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    if starring {
        format!("Could not star {name}: {reason}")
    } else {
        format!("Could not remove the star from {name}: {reason}")
    }
}

/// The folders in `star_folders.yaml`. An empty list when there is none.
fn read_folder_list(path: &Path) -> Result<BTreeSet<PathBuf>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(format!("{} could not be read ({e})", path.display())),
    };
    let list: FolderList =
        serde_yaml::from_str(&text).map_err(|e| format!("{} could not be read ({e})", path.display()))?;
    if list.version > VERSION {
        return Err(format!("{} was saved by a newer version of ViewSkater", path.display()));
    }
    Ok(list.folders)
}

/// Write the list through a temp file, like a star file. Paths that are
/// not valid UTF-8 cannot be written to YAML and are left out.
fn write_folder_list(path: &Path, folders: &BTreeSet<PathBuf>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let list = FolderList {
        version: VERSION,
        folders: folders.iter().filter(|f| f.to_str().is_some()).cloned().collect(),
    };
    let text = serde_yaml::to_string(&list).map_err(io::Error::other)?;
    let temp = path.with_extension("yaml.tmp");
    let mut file = fs::File::create(&temp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path)
}

/// The folder and the file name of `path`. The name is a key in YAML, so
/// it has to be valid UTF-8.
fn split(path: &Path) -> Result<(&Path, String), String> {
    let dir = path.parent().ok_or_else(|| "it is not in a folder".to_string())?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "its name is not valid UTF-8".to_string())?;
    Ok((dir, name.to_string()))
}

/// A name from the file that points at a file in the same folder, not at
/// a path somewhere else.
fn is_plain_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(std::path::Component::Normal(_))) && components.next().is_none()
}

fn read_folder(dir: &Path) -> FolderStars {
    match read_file(&dir.join(FILE_NAME)) {
        Ok(None) => FolderStars::default(),
        Ok(Some(file)) => {
            let mut folder = FolderStars::default();
            for (name, entry) in file.stars {
                if !is_plain_name(&name) {
                    continue;
                }
                let same_file = fs::metadata(dir.join(&name)).is_ok_and(|m| m.len() == entry.size);
                if same_file {
                    folder.starred.insert(name.clone());
                }
                folder.names_in_file.insert(name);
            }
            folder
        }
        Err(reason) => {
            log::warn!("Stars in {} are not shown: {reason}", dir.display());
            FolderStars { locked: Some(reason), ..Default::default() }
        }
    }
}

/// The folder's file, checked. `Ok(None)` when the folder has none.
fn read_file(path: &Path) -> Result<Option<StarFile>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{FILE_NAME} could not be read ({e})")),
    };
    let file: StarFile =
        serde_yaml::from_str(&text).map_err(|e| format!("{FILE_NAME} could not be read ({e})"))?;
    if file.version > VERSION {
        return Err(format!("{FILE_NAME} was saved by a newer version of ViewSkater"));
    }
    Ok(Some(file))
}

/// Apply one change to the folder's file as it is on disk now. Another
/// window or pane may have changed the file since it was loaded, so only
/// this entry changes. A file whose last entry goes stays, empty: the app
/// never unlinks a file (see the test in `trash_bin`).
fn save_change(dir: &Path, name: &str, change: &Change) -> Result<(), String> {
    let path = dir.join(FILE_NAME);
    let mut file = read_file(&path)?.unwrap_or(StarFile { version: VERSION, stars: BTreeMap::new() });
    match change {
        Change::Star(entry) => {
            file.stars.insert(name.to_string(), entry.clone());
        }
        Change::Unstar => {
            if file.stars.remove(name).is_none() {
                return Ok(());
            }
        }
    }
    file.version = VERSION;
    let text = serde_yaml::to_string(&file).map_err(|e| e.to_string())?;
    write_through_temp(dir, &path, &text).map_err(|e| e.to_string())
}

/// Write `text` to a hidden temp file and rename it over `path`. A crash
/// or a dropped connection in the middle leaves the old file whole.
fn write_through_temp(dir: &Path, path: &Path, text: &str) -> io::Result<()> {
    let temp = dir.join(TEMP_NAME);
    let mut file = create_hidden(&temp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path)
}

/// Windows refuses to open an existing hidden file for rewriting unless
/// the open asks for the same attribute: CreateFile with CREATE_ALWAYS
/// "fails and sets the last error to ERROR_ACCESS_DENIED if the file
/// exists and has the FILE_ATTRIBUTE_HIDDEN or FILE_ATTRIBUTE_SYSTEM
/// attribute". The leading dot hides the file everywhere else.
#[cfg(target_os = "windows")]
fn create_hidden(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .attributes(FILE_ATTRIBUTE_HIDDEN)
        .open(path)
}

#[cfg(not(target_os = "windows"))]
fn create_hidden(path: &Path) -> io::Result<fs::File> {
    fs::File::create(path)
}

#[cfg(test)]
mod tests;
