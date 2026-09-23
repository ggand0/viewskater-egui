use super::*;

use crate::settings::ImageDiscoveryOptions;

/// A folder with three files of 100, 101 and 102 bytes. Only the names
/// and sizes matter here, nothing decodes them.
fn folder() -> (tempfile::TempDir, Vec<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    let images = ["a.jpg", "b.jpg", "c.jpg"]
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let path = dir.path().join(name);
            fs::write(&path, vec![0u8; 100 + i]).unwrap();
            path
        })
        .collect();
    (dir, images)
}

fn size(path: &Path) -> u64 {
    fs::metadata(path).unwrap().len()
}

/// Stars loaded the way a folder listing loads them.
fn loaded(dir: &Path, images: &[PathBuf]) -> Stars {
    let star_file = dir.join(FILE_NAME);
    let star_files = if star_file.exists() { vec![star_file] } else { Vec::new() };
    let mut stars = Stars::new(&egui::Context::default(), None);
    stars.load(images, &star_files);
    stars
}

fn names_on_disk(dir: &Path) -> Vec<String> {
    let text = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
    let file: StarFile = serde_yaml::from_str(&text).unwrap();
    file.stars.into_keys().collect()
}

#[test]
fn stars_survive_a_reload() {
    let (dir, images) = folder();
    let mut stars = loaded(dir.path(), &images);
    stars.star(&images[0], size(&images[0])).unwrap();
    stars.star(&images[2], size(&images[2])).unwrap();
    assert!(stars.is_starred(&images[0]), "the star shows before the save");
    assert!(stars.wait_for_writes().is_empty());

    let text = fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
    let file: StarFile = serde_yaml::from_str(&text).unwrap();
    assert_eq!(file.version, 1);
    assert_eq!(file.stars.keys().collect::<Vec<_>>(), ["a.jpg", "c.jpg"]);
    assert_eq!(file.stars["a.jpg"].rating, 1);
    assert_eq!(file.stars["a.jpg"].size, 100);
    assert_eq!(file.stars["c.jpg"].size, 102);
    assert!(chrono::DateTime::parse_from_rfc3339(&file.stars["a.jpg"].changed).is_ok());
    assert!(!dir.path().join(TEMP_NAME).exists(), "the temp file was renamed away");

    let again = loaded(dir.path(), &images);
    assert!(again.is_starred(&images[0]));
    assert!(!again.is_starred(&images[1]));
    assert!(again.is_starred(&images[2]));
}

/// The app never unlinks a file, so the file stays when its last entry
/// goes, with no stars in it.
#[test]
fn removing_the_last_entry_leaves_an_empty_file() {
    let (dir, images) = folder();
    let mut stars = loaded(dir.path(), &images);
    stars.star(&images[1], size(&images[1])).unwrap();
    assert!(stars.wait_for_writes().is_empty());

    stars.unstar(&images[1]).unwrap();
    assert!(!stars.is_starred(&images[1]));
    assert!(stars.wait_for_writes().is_empty());
    assert!(names_on_disk(dir.path()).is_empty());
    assert!(!loaded(dir.path(), &images).is_starred(&images[1]));
}

/// Two windows on one folder, each with its own copy of the stars. A save
/// reads the file again first, so the second window does not write over
/// the first window's star.
#[test]
fn a_star_from_another_window_survives() {
    let (dir, images) = folder();
    let mut first = loaded(dir.path(), &images);
    let mut second = loaded(dir.path(), &images);
    first.star(&images[0], size(&images[0])).unwrap();
    assert!(first.wait_for_writes().is_empty());
    second.star(&images[1], size(&images[1])).unwrap();
    assert!(second.wait_for_writes().is_empty());
    assert_eq!(names_on_disk(dir.path()), ["a.jpg", "b.jpg"]);
}

#[test]
fn a_file_that_does_not_parse_is_never_written() {
    let (dir, images) = folder();
    let garbage = "stars: [a.jpg\n";
    fs::write(dir.path().join(FILE_NAME), garbage).unwrap();
    let mut stars = loaded(dir.path(), &images);

    let err = stars.star(&images[0], size(&images[0])).unwrap_err();
    assert!(err.starts_with("Could not star a.jpg: .viewskater.yaml could not be read"), "{err}");
    assert!(!stars.is_starred(&images[0]));
    assert!(stars.wait_for_writes().is_empty());
    assert_eq!(fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(), garbage);
}

#[test]
fn a_file_from_a_newer_version_is_never_written() {
    let (dir, images) = folder();
    let newer = "version: 2\nstars:\n  a.jpg:\n    rating: 1\n    size: 100\n    changed: x\n";
    fs::write(dir.path().join(FILE_NAME), newer).unwrap();
    let mut stars = loaded(dir.path(), &images);

    assert!(!stars.is_starred(&images[0]), "a file this build cannot write shows no stars");
    let err = stars.unstar(&images[0]).unwrap_err();
    assert!(err.contains("newer version of ViewSkater"), "{err}");
    assert!(stars.wait_for_writes().is_empty());
    assert_eq!(fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(), newer);
}

/// An entry whose file now has another size, or is gone, shows no star.
/// It stays in the file: a listing that skipped entries must not cost
/// photos their stars, and a file that comes back gets its star back.
#[test]
fn entries_for_changed_or_missing_files_stay_in_the_file() {
    let (dir, images) = folder();
    let file = "version: 1\nstars:\n  \
                a.jpg:\n    rating: 1\n    size: 999\n    changed: x\n  \
                b.jpg:\n    rating: 1\n    size: 101\n    changed: x\n  \
                gone.jpg:\n    rating: 1\n    size: 5\n    changed: x\n";
    fs::write(dir.path().join(FILE_NAME), file).unwrap();
    let mut stars = loaded(dir.path(), &images);
    assert!(!stars.is_starred(&images[0]), "another size is another photo");
    assert!(stars.is_starred(&images[1]));

    stars.star(&images[2], size(&images[2])).unwrap();
    stars.unstar(&images[1]).unwrap();
    assert!(stars.wait_for_writes().is_empty());
    assert_eq!(names_on_disk(dir.path()), ["a.jpg", "c.jpg", "gone.jpg"]);

    fs::write(dir.path().join("gone.jpg"), [0u8; 5]).unwrap();
    let mut images = images;
    images.push(dir.path().join("gone.jpg"));
    let again = loaded(dir.path(), &images);
    assert!(again.is_starred(&images[3]), "the file came back with the same size");
}

#[test]
fn names_that_point_outside_the_folder_are_ignored() {
    assert!(is_plain_name("a.jpg"));
    assert!(!is_plain_name("../a.jpg"));
    assert!(!is_plain_name("sub/a.jpg"));
    assert!(!is_plain_name("/etc/passwd"));
    assert!(!is_plain_name("."));
    assert!(!is_plain_name(""));

    let (dir, images) = folder();
    let outside = dir.path().parent().unwrap().join("outside.jpg");
    let file = format!(
        "version: 1\nstars:\n  ../outside.jpg:\n    rating: 1\n    size: {}\n    changed: x\n",
        fs::metadata(&images[0]).unwrap().len()
    );
    fs::write(dir.path().join(FILE_NAME), file).unwrap();
    let stars = loaded(dir.path(), &images);
    assert!(!stars.is_starred(&outside));
}

#[test]
fn trash_removes_the_entry() {
    let (dir, images) = folder();
    let mut stars = loaded(dir.path(), &images);
    stars.star(&images[0], size(&images[0])).unwrap();
    stars.star(&images[1], size(&images[1])).unwrap();
    assert!(stars.wait_for_writes().is_empty());

    stars.forget(&images[0]);
    assert!(stars.wait_for_writes().is_empty());
    assert_eq!(names_on_disk(dir.path()), ["b.jpg"]);

    stars.forget(&images[1]);
    assert!(stars.wait_for_writes().is_empty());
    assert!(names_on_disk(dir.path()).is_empty());
}

/// Moving an image without a star to the trash writes nothing, so a
/// folder without stars never gets the file.
#[test]
fn trash_without_a_star_writes_nothing() {
    let (dir, images) = folder();
    let mut stars = loaded(dir.path(), &images);
    stars.forget(&images[0]);
    assert!(stars.folders.values().all(|f| f.pending_writes == 0));
    assert!(!dir.path().join(FILE_NAME).exists());
}

#[cfg(unix)]
#[test]
fn a_failed_save_takes_the_star_back() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, images) = folder();
    let mut stars = loaded(dir.path(), &images);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();

    stars.star(&images[0], size(&images[0])).unwrap();
    assert!(stars.is_starred(&images[0]));
    let failures = stars.wait_for_writes();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].starts_with("Could not star a.jpg: "), "{}", failures[0]);
    assert!(!stars.is_starred(&images[0]));
    assert!(!dir.path().join(FILE_NAME).exists());
}

/// The file is hidden on Windows, and a second save replaces it. Opening
/// an existing hidden file for rewriting without the hidden attribute
/// fails there.
#[cfg(target_os = "windows")]
#[test]
fn the_file_is_hidden_and_a_second_save_replaces_it() {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;
    let (dir, images) = folder();
    let mut stars = loaded(dir.path(), &images);
    stars.star(&images[0], size(&images[0])).unwrap();
    assert!(stars.wait_for_writes().is_empty());
    let attributes = fs::metadata(dir.path().join(FILE_NAME)).unwrap().file_attributes();
    assert_ne!(attributes & FILE_ATTRIBUTE_HIDDEN, 0);

    stars.star(&images[1], size(&images[1])).unwrap();
    assert!(stars.wait_for_writes().is_empty());
    assert_eq!(names_on_disk(dir.path()), ["a.jpg", "b.jpg"]);
}

/// The listing walks past the hidden file of every folder it lists,
/// subfolders included, and never lists it as an image.
#[test]
fn the_listing_notes_the_star_files() {
    let (dir, _) = folder();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("d.jpg"), [0u8; 10]).unwrap();
    fs::write(dir.path().join(FILE_NAME), "version: 1\n").unwrap();
    fs::write(sub.join(FILE_NAME), "version: 1\n").unwrap();

    let opts = ImageDiscoveryOptions { recursive: true, ..Default::default() };
    let listing = crate::file_io::enumerate_images(dir.path(), opts);
    assert_eq!(listing.images.len(), 4);
    assert!(listing.images.iter().all(|p| p.file_name().unwrap() != FILE_NAME));
    let mut star_files = listing.star_files;
    star_files.sort();
    assert_eq!(star_files, [dir.path().join(FILE_NAME), sub.join(FILE_NAME)]);

    let with_hidden = ImageDiscoveryOptions { include_hidden: true, ..Default::default() };
    let listing = crate::file_io::enumerate_images(dir.path(), with_hidden);
    assert_eq!(listing.images.len(), 3, "the hidden file is no image even when hidden files are listed");
    assert_eq!(listing.star_files, [dir.path().join(FILE_NAME)]);
}

// ---- the list of folders and moving the files to the trash -------------

/// Stars with the folder list in `list`, loaded over `dir`.
fn loaded_with_list(dir: &Path, images: &[PathBuf], list: &Path) -> Stars {
    let star_file = dir.join(FILE_NAME);
    let star_files = if star_file.exists() { vec![star_file] } else { Vec::new() };
    let mut stars = Stars::new(&egui::Context::default(), Some(list.to_path_buf()));
    stars.load(images, &star_files);
    stars
}

fn listed_on_disk(list: &Path) -> Vec<PathBuf> {
    read_folder_list(list).unwrap().into_iter().collect()
}

/// A move to the trash that renames the file into `bin` under a numbered
/// name, since every star file has the same name. Nothing touches the
/// real trash.
fn move_into(bin: &Path) -> impl Fn(&Path) -> Result<(), String> + '_ {
    let moved = std::cell::Cell::new(0);
    move |path: &Path| {
        moved.set(moved.get() + 1);
        let target = bin.join(format!("{}{}", moved.get(), path.file_name().unwrap().to_string_lossy()));
        fs::rename(path, target).map_err(|e| e.to_string())
    }
}

#[test]
fn starring_puts_the_folder_in_the_list() {
    let (dir, images) = folder();
    let config = tempfile::tempdir().unwrap();
    let list = config.path().join("star_folders.yaml");
    let mut stars = loaded_with_list(dir.path(), &images, &list);
    assert!(stars.star_file_folders().is_empty());

    stars.star(&images[0], size(&images[0])).unwrap();
    assert!(stars.wait_for_writes().is_empty());
    assert_eq!(listed_on_disk(&list), [dir.path().to_path_buf()]);

    let again = Stars::new(&egui::Context::default(), Some(list.clone()));
    assert_eq!(again.star_file_folders().iter().collect::<Vec<_>>(), [dir.path()]);
}

/// A file from before the list existed, from another computer or in a
/// copied folder joins the list when a listing walks past it.
#[test]
fn a_listed_folder_with_a_file_joins_the_list() {
    let (dir, images) = folder();
    fs::write(dir.path().join(FILE_NAME), "version: 1\n").unwrap();
    let config = tempfile::tempdir().unwrap();
    let list = config.path().join("star_folders.yaml");
    let stars = loaded_with_list(dir.path(), &images, &list);
    assert_eq!(stars.star_file_folders().iter().collect::<Vec<_>>(), [dir.path()]);
    assert_eq!(listed_on_disk(&list), [dir.path().to_path_buf()]);
}

#[test]
fn a_folder_another_window_listed_stays_in_the_list() {
    let (first_dir, first_images) = folder();
    let (second_dir, second_images) = folder();
    let config = tempfile::tempdir().unwrap();
    let list = config.path().join("star_folders.yaml");
    let mut first = loaded_with_list(first_dir.path(), &first_images, &list);
    let mut second = loaded_with_list(second_dir.path(), &second_images, &list);

    first.star(&first_images[0], size(&first_images[0])).unwrap();
    second.star(&second_images[0], size(&second_images[0])).unwrap();
    assert!(first.wait_for_writes().is_empty());
    assert!(second.wait_for_writes().is_empty());

    let mut expected = vec![first_dir.path().to_path_buf(), second_dir.path().to_path_buf()];
    expected.sort();
    assert_eq!(listed_on_disk(&list), expected);
}

#[test]
fn a_list_from_a_newer_version_is_never_written() {
    let (dir, images) = folder();
    let config = tempfile::tempdir().unwrap();
    let list = config.path().join("star_folders.yaml");
    let newer = "version: 2\nfolders:\n- /somewhere\n";
    fs::write(&list, newer).unwrap();
    let mut stars = loaded_with_list(dir.path(), &images, &list);

    stars.star(&images[0], size(&images[0])).unwrap();
    assert!(stars.wait_for_writes().is_empty());
    assert_eq!(fs::read_to_string(&list).unwrap(), newer);
    assert!(stars.star_file_folders().contains(dir.path()), "listed in memory");
}

/// Two folders with stars and one listed folder whose file is gone. Both
/// files go into the bin, never deleted, and the list ends up empty.
#[test]
fn every_star_file_goes_to_the_trash() {
    let (first_dir, first_images) = folder();
    let (second_dir, second_images) = folder();
    let (gone_dir, gone_images) = folder();
    let config = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let list = config.path().join("star_folders.yaml");
    let mut stars = Stars::new(&egui::Context::default(), Some(list.clone()));
    let all_images: Vec<PathBuf> =
        first_images.iter().chain(&second_images).chain(&gone_images).cloned().collect();
    stars.load(&all_images, &[]);
    stars.star(&first_images[0], size(&first_images[0])).unwrap();
    stars.star(&second_images[1], size(&second_images[1])).unwrap();
    stars.star(&gone_images[2], size(&gone_images[2])).unwrap();
    assert!(stars.wait_for_writes().is_empty());
    fs::rename(gone_dir.path().join(FILE_NAME), bin.path().join("taken away")).unwrap();

    let moves = stars.move_star_files(move_into(bin.path()), |_| false);

    assert_eq!(moves.moved, 2);
    assert!(moves.left_in_place.is_empty());
    assert!(moves.failed.is_empty());
    assert!(!first_dir.path().join(FILE_NAME).exists());
    assert!(!second_dir.path().join(FILE_NAME).exists());
    assert_eq!(fs::read_dir(bin.path()).unwrap().count(), 3, "two moved files and the one taken away");
    assert!(!stars.is_starred(&first_images[0]));
    assert!(!stars.is_starred(&second_images[1]));
    assert!(stars.star_file_folders().is_empty());
    assert!(listed_on_disk(&list).is_empty());
}

/// On a Windows network share or removable drive the trash would delete
/// for good. Those files stay, keep their stars and stay in the list.
#[test]
fn a_file_where_the_trash_would_delete_stays() {
    let (dir, images) = folder();
    let config = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let list = config.path().join("star_folders.yaml");
    let mut stars = loaded_with_list(dir.path(), &images, &list);
    stars.star(&images[0], size(&images[0])).unwrap();
    assert!(stars.wait_for_writes().is_empty());

    let moves = stars.move_star_files(move_into(bin.path()), |_| true);

    assert_eq!(moves.moved, 0);
    assert_eq!(moves.left_in_place, [dir.path().join(FILE_NAME)]);
    assert!(dir.path().join(FILE_NAME).exists());
    assert!(stars.is_starred(&images[0]));
    assert_eq!(listed_on_disk(&list), [dir.path().to_path_buf()]);
}

#[test]
fn a_refused_move_keeps_the_file_and_its_stars() {
    let (dir, images) = folder();
    let config = tempfile::tempdir().unwrap();
    let list = config.path().join("star_folders.yaml");
    let mut stars = loaded_with_list(dir.path(), &images, &list);
    stars.star(&images[0], size(&images[0])).unwrap();
    assert!(stars.wait_for_writes().is_empty());

    let moves = stars.move_star_files(|_| Err("no trash here".to_string()), |_| false);

    assert_eq!(moves.moved, 0);
    assert_eq!(moves.failed, [(dir.path().join(FILE_NAME), "no trash here".to_string())]);
    assert!(dir.path().join(FILE_NAME).exists());
    assert!(stars.is_starred(&images[0]));
    assert_eq!(listed_on_disk(&list), [dir.path().to_path_buf()]);
}
