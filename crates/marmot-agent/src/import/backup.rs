//! The backup of an old account home, taken before the core first opens its store (D-089).
//!
//! MDK upgrades its storage forward only, so this copy is the only way back. It is the whole home,
//! copied to a dated folder beside it that only its owner can read, and checked byte for byte
//! after the copy. Links are copied as links, never followed. A socket or other special file is
//! a running program's, not state, and is left out.
//!
//! Nothing half done is ever under a final name: the copy is made in a private folder beside the
//! home, put on disk, checked, and only then renamed into place, never over anything already
//! there; where the file system cannot rename without the risk of replacing, a file is put in
//! place by a link that is made only where nothing is, and a folder is not put in place at all.
//! The settings file is published the same way.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// What a backup holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Copied {
    /// Files copied and checked.
    pub files: u64,
    /// Their bytes.
    pub bytes: u64,
    /// Links copied as links.
    pub links: u64,
    /// Sockets and other special files left out.
    pub left_out: u64,
}

/// Why a backup or a file could not be put in place.
#[derive(Debug)]
pub enum Failed {
    /// Nothing is under the final name, and what was made on the way is removed.
    NotMade(io::Error),
    /// It is in place under its final name, but whether that reached the disk could not be
    /// confirmed.
    Unconfirmed(io::Error),
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotMade(e) | Self::Unconfirmed(e) => write!(f, "{e}"),
        }
    }
}

/// Where the backup of `home` goes: beside it, named after it and the time, such as
/// `codex.backup-20261121T093000Z`.
pub fn place_for(home: &Path, at: SystemTime) -> PathBuf {
    let name = home
        .file_name()
        .map_or_else(|| "home".into(), |n| n.to_string_lossy().into_owned());
    let stamp = utc_stamp(at);
    home.with_file_name(format!("{name}.backup-{stamp}"))
}

/// Copies `home` to `to`, which must not exist, owner-only, and checks the copy. The copy is
/// made beside `to` under a name of its own, and removed if anything fails before it is in place.
pub fn take(home: &Path, to: &Path) -> Result<Copied, Failed> {
    if fs::symlink_metadata(to).is_ok() {
        return Err(Failed::NotMade(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the backup folder is already there",
        )));
    }
    let partial = partial_beside(to).map_err(Failed::NotMade)?;
    // Opened before anything is put in place, so the one step left afterwards is the sync.
    let folder = folder_of(to).map_err(Failed::NotMade)?;
    let made = (|| {
        let mut copied = Copied::default();
        copy_folder(home, &partial, &mut copied)?;
        let checked = check(home, &partial)?;
        if checked != copied {
            return Err(io::Error::other("the backup does not match the home"));
        }
        rename_new(&partial, to, true)?;
        Ok(copied)
    })();
    let copied = made.map_err(|e| {
        let _ = fs::remove_dir_all(&partial);
        Failed::NotMade(e)
    })?;
    folder.sync_all().map_err(Failed::Unconfirmed)?;
    Ok(copied)
}

/// Writes `bytes` to `to`, owner-only, through a file of its own beside it that is put on disk
/// first, and never over a file already there.
pub fn publish_file(to: &Path, bytes: &[u8]) -> Result<(), Failed> {
    let partial = partial_beside(to).map_err(Failed::NotMade)?;
    let folder = folder_of(to).map_err(Failed::NotMade)?;
    let made = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&partial)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        rename_new(&partial, to, false)
    })();
    made.map_err(|e| {
        let _ = fs::remove_file(&partial);
        Failed::NotMade(e)
    })?;
    folder.sync_all().map_err(Failed::Unconfirmed)
}

/// What a partial copy of `to` is called: hidden, beside it, and unique to this run, so one an
/// earlier run left never stands in the way.
fn partial_beside(to: &Path) -> io::Result<PathBuf> {
    let name = to
        .file_name()
        .ok_or_else(|| io::Error::other("the path has no name"))?
        .to_string_lossy();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    Ok(to.with_file_name(format!(".{name}.partial-{}-{nanos}", std::process::id())))
}

/// Partial backups of `home` an earlier import left behind, for the person to remove.
pub fn partials_of(home: &Path) -> Vec<PathBuf> {
    let (Some(parent), Some(name)) = (home.parent(), home.file_name()) else {
        return Vec::new();
    };
    let prefix = format!(".{}.backup-", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with(&prefix) && name.contains(".partial-")
        })
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

/// Renames `from`, a folder when `folder` is set and a file otherwise, to `to`, failing if `to`
/// is there.
fn rename_new(from: &Path, to: &Path, folder: bool) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        // A file system, or a system, that cannot refuse to replace.
        Err(e) if e == rustix::io::Errno::INVAL || e == rustix::io::Errno::NOSYS => {
            put_in_place_without_rename(from, to, folder)
        }
        Err(e) => Err(e.into()),
    }
}

/// Where renaming cannot refuse to replace: looking first and then renaming would replace what
/// appeared in between, so a file is put in place by a link, which is made only where nothing
/// is, and a folder is not put in place at all.
fn put_in_place_without_rename(from: &Path, to: &Path, folder: bool) -> io::Result<()> {
    if folder {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this file system cannot rename a folder without the risk of replacing another",
        ));
    }
    fs::hard_link(from, to)?;
    // In place already; a partial name left behind is only untidy.
    let _ = fs::remove_file(from);
    Ok(())
}

/// The folder `path` is in, open; `.` for a bare name.
fn folder_of(path: &Path) -> io::Result<fs::File> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => fs::File::open(parent),
        Some(_) => fs::File::open("."),
        None => Err(io::Error::other("the path has no folder")),
    }
}

fn new_private_folder(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)?;
    // The umask may have taken bits away, never added them; set it exactly.
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn copy_folder(from: &Path, to: &Path, copied: &mut Copied) -> io::Result<()> {
    new_private_folder(to)?;
    for entry in sorted_entries(from)? {
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = fs::symlink_metadata(&source)?.file_type();
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&source)?, &target)?;
            copied.links += 1;
        } else if kind.is_dir() {
            copy_folder(&source, &target, copied)?;
        } else if kind.is_file() {
            copied.bytes += copy_file(&source, &target)?;
            copied.files += 1;
        } else {
            copied.left_out += 1;
        }
    }
    // The folder's entries, on disk.
    fs::File::open(to)?.sync_all()
}

/// Copies one file, readable and writable by its owner alone, and makes sure it is on disk.
fn copy_file(from: &Path, to: &Path) -> io::Result<u64> {
    let mode = fs::metadata(from)?.permissions().mode() & 0o700;
    let mut source = fs::File::open(from)?;
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)?;
    let bytes = io::copy(&mut source, &mut target)?;
    target.sync_all()?;
    fs::set_permissions(to, fs::Permissions::from_mode(mode | 0o600))?;
    Ok(bytes)
}

/// Walks the home and the copy side by side, and gives what the copy holds when every file,
/// link and folder in one is in the other, the same.
pub fn check(home: &Path, copy: &Path) -> io::Result<Copied> {
    let mut seen = Copied::default();
    check_folder(home, copy, &mut seen)?;
    Ok(seen)
}

fn differs(what: &str) -> io::Error {
    io::Error::other(format!("the backup differs from the home: {what}"))
}

fn check_folder(home: &Path, copy: &Path, seen: &mut Copied) -> io::Result<()> {
    if fs::symlink_metadata(copy)?.permissions().mode() & 0o077 != 0 {
        return Err(differs("a folder others can read"));
    }
    // What the copy should hold, everything but special files, against what it holds: both in
    // name order, so one pass finds an entry missing or added.
    let mut kept = Vec::new();
    for entry in sorted_entries(home)? {
        let kind = fs::symlink_metadata(entry.path())?.file_type();
        if kind.is_symlink() || kind.is_dir() || kind.is_file() {
            kept.push((entry, kind));
        } else {
            seen.left_out += 1;
        }
    }
    let copies = sorted_entries(copy)?;
    if kept.len() != copies.len()
        || kept
            .iter()
            .zip(&copies)
            .any(|((a, _), b)| a.file_name() != b.file_name())
    {
        return Err(differs("an entry missing, or one the home does not have"));
    }
    for (entry, kind) in kept {
        let source = entry.path();
        let target = copy.join(entry.file_name());
        let target_kind = fs::symlink_metadata(&target)?.file_type();
        if kind.is_symlink() {
            if !target_kind.is_symlink() || fs::read_link(&source)? != fs::read_link(&target)? {
                return Err(differs("a link"));
            }
            seen.links += 1;
        } else if kind.is_dir() {
            if !target_kind.is_dir() {
                return Err(differs("a folder"));
            }
            check_folder(&source, &target, seen)?;
        } else {
            if !target_kind.is_file() {
                return Err(differs("a file"));
            }
            if fs::metadata(&target)?.permissions().mode() & 0o077 != 0 {
                return Err(differs("a file others can read"));
            }
            seen.bytes += same_bytes(&source, &target)?;
            seen.files += 1;
        }
    }
    Ok(())
}

/// Compares two files byte for byte, and gives their length.
fn same_bytes(a: &Path, b: &Path) -> io::Result<u64> {
    let mut a = fs::File::open(a)?;
    let mut b = fs::File::open(b)?;
    let mut left = vec![0u8; 64 * 1024];
    let mut right = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = read_full(&mut a, &mut left)?;
        let m = read_full(&mut b, &mut right)?;
        if n != m || left[..n] != right[..m] {
            return Err(differs("a file's contents"));
        }
        if n == 0 {
            return Ok(total);
        }
        total += n as u64;
    }
}

fn read_full(file: &mut fs::File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn sorted_entries(folder: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(folder)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    Ok(entries)
}

/// The time as `YYYYMMDDTHHMMSSZ`, in UTC.
pub fn utc_stamp(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The date of a day counted from 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_stamp_is_the_utc_time() {
        let at = UNIX_EPOCH + Duration::from_secs(1_795_253_400);
        assert_eq!(utc_stamp(at), "20261121T093000Z");
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101T000000Z");
        // A leap day.
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(utc_stamp(leap), "20000229T000000Z");
    }

    #[test]
    fn a_backup_is_a_checked_private_copy_that_leaves_sockets_out() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        fs::create_dir_all(home.join("accounts/a")).unwrap();
        fs::write(home.join("accounts/a/account.json"), b"{}").unwrap();
        fs::write(home.join("store.sqlite"), vec![7u8; 200_000]).unwrap();
        fs::set_permissions(home.join("store.sqlite"), fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink("store.sqlite", home.join("link")).unwrap();
        fs::create_dir(home.join("dev")).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(home.join("dev/s.sock")).unwrap();

        let to = place_for(&home, UNIX_EPOCH);
        assert_eq!(to, root.path().join("codex.backup-19700101T000000Z"));
        let copied = take(&home, &to).unwrap();
        assert_eq!(
            copied,
            Copied {
                files: 2,
                bytes: 200_002,
                links: 1,
                left_out: 1
            }
        );
        let mode = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&to), 0o700);
        assert_eq!(mode(&to.join("store.sqlite")), 0o600);
        assert_eq!(
            fs::read_link(to.join("link")).unwrap(),
            Path::new("store.sqlite")
        );
        assert!(!to.join("dev/s.sock").exists());

        // A second backup to the same place is refused, and a changed copy fails the check.
        assert!(take(&home, &to).is_err());
        fs::write(to.join("store.sqlite"), vec![8u8; 200_000]).unwrap();
        assert!(check(&home, &to).is_err());
    }

    #[test]
    fn a_failed_backup_leaves_nothing_and_a_crashed_one_does_not_block_the_next() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        fs::create_dir(&home).unwrap();
        fs::write(home.join("readable"), b"a").unwrap();
        fs::write(home.join("unreadable"), b"b").unwrap();
        fs::set_permissions(home.join("unreadable"), fs::Permissions::from_mode(0o000)).unwrap();
        let to = place_for(&home, UNIX_EPOCH);
        if fs::File::open(home.join("unreadable")).is_ok() {
            return; // Run with powers that read anything, the failure cannot be made.
        }
        assert!(take(&home, &to).is_err());
        assert!(!to.exists());
        assert!(partials_of(&home).is_empty(), "the partial copy is removed");

        // What a crash would leave: a partial copy. It does not stand in the way, and is listed.
        fs::set_permissions(home.join("unreadable"), fs::Permissions::from_mode(0o600)).unwrap();
        let crashed = partial_beside(&to).unwrap();
        fs::create_dir(&crashed).unwrap();
        take(&home, &to).unwrap();
        assert_eq!(partials_of(&home), [crashed]);
    }

    #[test]
    fn a_published_file_never_replaces_one() {
        let root = tempfile::tempdir().unwrap();
        let to = root.path().join("marmot-agent.toml");
        publish_file(&to, b"one").unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"one");
        assert_eq!(
            fs::metadata(&to).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let Failed::NotMade(again) = publish_file(&to, b"two").unwrap_err() else {
            panic!("published over the file");
        };
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"one");
        let left: Vec<_> = fs::read_dir(root.path()).unwrap().collect();
        assert_eq!(left.len(), 1, "no partial file is left");
    }

    /// Where renaming cannot refuse to replace, nothing already there is replaced: a file goes
    /// in by a link, which fails on anything there, and a folder does not go in at all.
    #[test]
    fn without_a_safe_rename_nothing_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("partial"), root.path().join("final"));
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"there first").unwrap();
        let err = put_in_place_without_rename(&from, &to, false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"there first");
        fs::remove_file(&to).unwrap();
        put_in_place_without_rename(&from, &to, false).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"new");
        assert!(!from.exists());

        let (from, to) = (root.path().join("dir.partial"), root.path().join("dir"));
        fs::create_dir(&from).unwrap();
        let err = put_in_place_without_rename(&from, &to, true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(!to.exists());
    }

    /// A bare file name is in the working folder, which is what is put on disk after it.
    #[test]
    fn a_bare_name_is_in_the_working_folder() {
        assert!(folder_of(Path::new("imported.toml")).is_ok());
        assert!(folder_of(Path::new("/")).is_err());
    }

    #[test]
    fn a_copy_with_an_entry_missing_or_added_fails_the_check() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        fs::create_dir(&home).unwrap();
        fs::write(home.join("a"), b"a").unwrap();
        fs::write(home.join("b"), b"b").unwrap();
        let to = place_for(&home, UNIX_EPOCH);
        take(&home, &to).unwrap();
        fs::write(to.join("c"), b"c").unwrap();
        fs::set_permissions(to.join("c"), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(check(&home, &to).is_err(), "an entry added");
        fs::remove_file(to.join("c")).unwrap();
        check(&home, &to).unwrap();
        fs::remove_file(to.join("b")).unwrap();
        assert!(check(&home, &to).is_err(), "an entry missing");
    }
}
