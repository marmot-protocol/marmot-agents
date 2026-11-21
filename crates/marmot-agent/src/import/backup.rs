//! The backup of an old account home, taken before the core first opens its store (D-089).
//!
//! MDK upgrades its storage forward only, so this copy is the only way back. It is the whole home,
//! copied to a dated folder beside it that only its owner can read, and checked byte for byte
//! after the copy. Links are copied as links, never followed. A socket or other special file is
//! a running program's, not state, and is left out.
//!
//! Nothing half done is ever under a final name: the copy is made in a private folder beside the
//! home, put on disk, checked, and only then renamed into place, never over anything already
//! there. The settings file is published the same way.

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
/// made beside `to` under a name of its own, and removed if anything fails.
pub fn take(home: &Path, to: &Path) -> io::Result<Copied> {
    if fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the backup folder is already there",
        ));
    }
    let partial = partial_beside(to)?;
    let made = (|| {
        let mut copied = Copied::default();
        copy_folder(home, &partial, &mut copied)?;
        let checked = check(home, &partial)?;
        if checked != copied {
            return Err(io::Error::other("the backup does not match the home"));
        }
        rename_new(&partial, to)?;
        sync_parent(to)?;
        Ok(copied)
    })();
    if made.is_err() {
        let _ = fs::remove_dir_all(&partial);
    }
    made
}

/// Writes `bytes` to `to`, owner-only, through a file of its own beside it that is put on disk
/// first, and never over a file already there.
pub fn publish_file(to: &Path, bytes: &[u8]) -> io::Result<()> {
    let partial = partial_beside(to)?;
    let made = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&partial)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        rename_new(&partial, to)?;
        sync_parent(to)
    })();
    if made.is_err() {
        let _ = fs::remove_file(&partial);
    }
    made
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

/// Renames `from` to `to`, failing if `to` is there.
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        // A file system that cannot refuse to replace: look first. A folder is never renamed
        // over one that holds anything.
        Err(e) if e == rustix::io::Errno::INVAL || e == rustix::io::Errno::NOSYS => {
            if fs::symlink_metadata(to).is_ok() {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            fs::rename(from, to)
        }
        Err(e) => Err(e.into()),
    }
}

/// Puts the entry for `path` in its folder on disk.
fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("the path has no folder"))?;
    fs::File::open(parent)?.sync_all()
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
    let originals = sorted_entries(home)?;
    let mut copies: Vec<_> = sorted_entries(copy)?
        .into_iter()
        .map(|e| e.file_name())
        .collect();
    for entry in originals {
        let source = entry.path();
        let target = copy.join(entry.file_name());
        let kind = fs::symlink_metadata(&source)?.file_type();
        let special = !(kind.is_symlink() || kind.is_dir() || kind.is_file());
        if special {
            seen.left_out += 1;
            continue;
        }
        copies.retain(|name| *name != entry.file_name());
        let target_kind = fs::symlink_metadata(&target)
            .map_err(|_| differs("a missing entry"))?
            .file_type();
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
    if !copies.is_empty() {
        return Err(differs("an entry the home does not have"));
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
        let again = publish_file(&to, b"two").unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"one");
        let left: Vec<_> = fs::read_dir(root.path()).unwrap().collect();
        assert_eq!(left.len(), 1, "no partial file is left");
    }
}
