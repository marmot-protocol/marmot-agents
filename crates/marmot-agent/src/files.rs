//! Files the agent sends to a chat (D-090).
//!
//! A driver hands the core a path; the core reads the file only if it is in one of the folders the
//! settings allow, by default the harness's working folder. Each folder is opened once, when the
//! core starts, and a path is followed from that open folder one part at a time, never following
//! a link the system would follow on its own. A link on the way is read and followed by its words,
//! from where it is, by the same rule, so a link may lead elsewhere inside the folder but never
//! out of it, and a `..` never climbs above it. Because each step starts from a folder already
//! open, a folder on the way swapped for a link while the path is followed cannot lead out
//! either. A file that also has a name somewhere else (a hard link), which could be outside, is
//! refused. Only an ordinary file is read, never a folder, a device or a pipe, and only up to
//! [`MAX_FILE`].
//!
//! No file inside the account home, which holds the agent's keys, is sent, whichever allowed folder
//! leads to it, unless the owner allows it in the settings (D-099). A file is inside the home when
//! the home is one of the folders on its way, known by its device and inode, or when the allowed
//! folder itself lies inside the home.
//!
//! Nothing here prints a path, a name or a file's contents (D-071). A refusal says why in words
//! the chat can read, and names no path.

use crate::config::Config;
use marmot_agent_driver::{Reason, Refusal};
use rustix::fs::{Mode, OFlags};
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// The largest file the agent may send: 64 MiB. MDK uploads a file it is handed in one piece,
/// held in memory, up to 512 MiB; this keeps well below that, and matches what today's Codex
/// connector takes in one batch.
pub const MAX_FILE: u64 = 64 * 1024 * 1024;

/// The most links one path may pass through, as the system allows.
const MAX_LINKS: usize = 40;

/// The most folders the way up from an allowed folder to the top is walked; a deeper one counts as
/// inside the home.
const MAX_DEPTH: usize = 4096;

/// A file read for sending.
pub struct Outgoing {
    /// The name people see: the last part of the path the agent gave.
    pub name: String,
    /// Its media type, from the name's extension; `application/octet-stream` when unknown.
    pub media_type: String,
    /// What it holds.
    pub bytes: Vec<u8>,
}

/// Shows the size, and not the name or what it holds (D-071).
impl std::fmt::Debug for Outgoing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outgoing")
            .field("size", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// One folder the agent may send files from: the folder itself, held open, and the names a path
/// may reach it by, as the settings give it and as its real path.
struct Root {
    names: Vec<PathBuf>,
    dir: File,
    /// Whether the folder is the account home or lies inside it, when files from there are kept.
    in_home: bool,
}

/// A folder as the system knows it, whatever its name: its device and inode.
type Id = (u64, u64);

/// The folders the agent may send files from.
#[derive(Clone, Default)]
pub struct Folders {
    allowed: Arc<Vec<Root>>,
    /// The account home, when no file inside it may be sent.
    home: Option<Id>,
}

/// Shows how many folders there are, and no path (D-071).
impl std::fmt::Debug for Folders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Folders")
            .field("allowed", &self.allowed.len())
            .finish()
    }
}

fn refused(reason: Reason, detail: &str) -> Refusal {
    Refusal {
        reason,
        detail: Some(detail.into()),
    }
}

fn outside() -> Refusal {
    refused(
        Reason::NotAllowed,
        "The file is outside the folders the agent may send files from.",
    )
}

fn in_home() -> Refusal {
    refused(
        Reason::NotAllowed,
        "The file is in the agent's own home, which holds its keys.",
    )
}

fn unreadable() -> Refusal {
    refused(Reason::Failed, "The file could not be read.")
}

/// Checks the folders the settings list for sending files against the account home, which holds
/// the agent's keys: none may be the home, hold it or lie inside it. Each is compared by its real
/// path, links followed, as [`Folders::new`] opens it; one that is not there allows nothing
/// anyway. The refusal names no path (D-071). Not checked when the owner allows files from the
/// home (D-099).
pub fn apart_from_home(folders: &[PathBuf], home: &Path) -> Result<(), String> {
    let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    for folder in folders {
        let Ok(real) = std::fs::canonicalize(folder) else {
            continue;
        };
        if real.starts_with(&home) || home.starts_with(&real) {
            return Err(
                "a folder in `files_from` is the account home, holds it or lies inside it, and \
                 the home holds the agent's keys; take that folder out of the settings, or, to \
                 let the agent send files from its home, add `send_files_from_agent_home = true`"
                    .into(),
            );
        }
    }
    Ok(())
}

impl Folders {
    /// The folders the settings allow, kept apart from the account home unless the owner allows
    /// files from it (D-099). A folder listed in `files_from` that is, holds or lies inside the
    /// home is refused, as [`apart_from_home`] says.
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let home = (!config.send_files_from_agent_home).then_some(config.home.as_path());
        if let (Some(home), Some(listed)) = (home, &config.files_from) {
            apart_from_home(listed, home)?;
        }
        Ok(Self::new(&config.file_folders(), home))
    }

    /// The folders, each opened now. One that does not exist, or cannot be opened, allows
    /// nothing. With `home`, no file inside it is sent, whichever folder leads to it.
    pub fn new(folders: &[PathBuf], home: Option<&Path>) -> Self {
        // A home that is not there holds nothing.
        let home = home.and_then(id_of_path);
        let allowed = folders
            .iter()
            .filter_map(|folder| {
                let real = std::fs::canonicalize(folder).ok()?;
                let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
                let dir = File::from(rustix::fs::open(&real, flags, Mode::empty()).ok()?);
                // Asked of the folder held open, not of its name, which may since lead elsewhere.
                let in_home = home.is_some_and(|home| lies_in(&dir, home));
                let mut names = vec![real];
                if let Ok(given) = std::path::absolute(folder)
                    && !names.contains(&given)
                {
                    names.push(given);
                }
                Some(Root {
                    names,
                    dir,
                    in_home,
                })
            })
            .collect();
        Self {
            allowed: Arc::new(allowed),
            home,
        }
    }

    /// Reads the file at `path` for sending, or says why it may not be sent.
    pub fn read(&self, path: &Path) -> Result<Outgoing, Refusal> {
        if self.allowed.is_empty() {
            return Err(refused(
                Reason::NotSupported,
                "No folder is set for the agent to send files from.",
            ));
        }
        if !path.is_absolute() {
            return Err(refused(
                Reason::Invalid,
                "The agent did not give a whole path.",
            ));
        }
        let mut first = None;
        for root in self.allowed.iter() {
            let Some(rest) = root.names.iter().find_map(|n| path.strip_prefix(n).ok()) else {
                continue;
            };
            match root.open(rest, self.home) {
                Ok(file) => return finish(file, path),
                Err(refusal) => {
                    first.get_or_insert(refusal);
                }
            }
        }
        Err(first.unwrap_or_else(outside))
    }
}

impl Root {
    /// Opens `rest`, a path inside this folder, one part at a time from the folder held open. With
    /// `home`, a file with the home on its way is refused.
    fn open(&self, rest: &Path, home: Option<Id>) -> Result<File, Refusal> {
        // The folders opened on the way, below this one; a `..` goes back to the one before.
        let mut opened: Vec<File> = Vec::new();
        let mut parts: VecDeque<OsString> = VecDeque::new();
        push_front(&mut parts, rest);
        let mut links = 0;
        while let Some(part) = parts.pop_front() {
            if part == ".." {
                if opened.pop().is_none() {
                    return Err(outside());
                }
                continue;
            }
            let at: BorrowedFd<'_> = opened.last().map_or(self.dir.as_fd(), |d| d.as_fd());
            let last = parts.is_empty();
            // A link is never followed by the system here, and a pipe is never waited on.
            let flags = if last {
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC
            } else {
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
            };
            let error = match rustix::fs::openat(at, part.as_os_str(), flags, Mode::empty()) {
                Ok(fd) if last => {
                    // The folders still open are the file's own, from this one down, since a
                    // `..` closes the last and a whole link starts again from this one.
                    // One whose identity the system does not give counts as the home.
                    if let Some(home) = home
                        && (self.in_home
                            || opened.iter().any(|d| id_of(d).is_none_or(|i| i == home)))
                    {
                        return Err(in_home());
                    }
                    return Ok(File::from(fd));
                }
                Ok(fd) => {
                    opened.push(File::from(fd));
                    continue;
                }
                Err(error) => error,
            };
            // It may be a link: read it, and follow its words from here, one part at a time.
            let Ok(target) = rustix::fs::readlinkat(at, part.as_os_str(), Vec::new()) else {
                return Err(match error {
                    rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR => {
                        refused(Reason::Invalid, "There is no such file.")
                    }
                    _ => unreadable(),
                });
            };
            links += 1;
            if links > MAX_LINKS {
                return Err(refused(Reason::Invalid, "The path passes too many links."));
            }
            let target = PathBuf::from(OsString::from_vec(target.into_bytes()));
            if target.is_absolute() {
                // A whole path counts only if it names this folder; it starts again from there.
                let Some(inside) = self.names.iter().find_map(|n| target.strip_prefix(n).ok())
                else {
                    return Err(outside());
                };
                opened.clear();
                push_front(&mut parts, inside);
            } else {
                push_front(&mut parts, &target);
            }
        }
        // The path ends at a folder.
        Err(refused(
            Reason::Invalid,
            "Only an ordinary file can be sent.",
        ))
    }
}

/// The device and inode of an open file, if the system says.
fn id_of(file: &File) -> Option<Id> {
    let meta = file.metadata().ok()?;
    Some((meta.dev(), meta.ino()))
}

/// Whether the folder held open is `home` or lies inside it, found from the folder itself by going
/// up one `..` at a time to the top, so a rename of its name or of a folder above it cannot make
/// the answer describe another folder. When the way up cannot be followed, it counts as inside.
fn lies_in(dir: &File, home: Id) -> bool {
    // Only the folder's identity is needed on the way, so where the system allows, the folders
    // above are opened without needing leave to read them.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let Some(mut id) = id_of(dir) else {
        return true;
    };
    let mut here: Option<File> = None;
    for _ in 0..MAX_DEPTH {
        if id == home {
            return true;
        }
        let at = here.as_ref().unwrap_or(dir).as_fd();
        let Ok(up) = rustix::fs::openat(at, "..", flags, Mode::empty()) else {
            return true;
        };
        let up = File::from(up);
        let Some(above) = id_of(&up) else {
            return true;
        };
        if above == id {
            // The top, which is its own `..`.
            return false;
        }
        id = above;
        here = Some(up);
    }
    true
}

/// The device and inode of the folder at `path`, links followed.
fn id_of_path(path: &Path) -> Option<Id> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// Puts the parts of `path` in front of those still to follow.
fn push_front(parts: &mut VecDeque<OsString>, path: &Path) {
    let names: Vec<&OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name),
            Component::ParentDir => Some(OsStr::new("..")),
            _ => None,
        })
        .collect();
    for name in names.into_iter().rev() {
        parts.push_front(name.to_os_string());
    }
}

/// Reads a file opened inside a folder, if it is one that may be sent.
fn finish(mut file: File, path: &Path) -> Result<Outgoing, Refusal> {
    let meta = file.metadata().map_err(|_| unreadable())?;
    if !meta.is_file() {
        return Err(refused(
            Reason::Invalid,
            "Only an ordinary file can be sent.",
        ));
    }
    if meta.nlink() > 1 {
        return Err(refused(
            Reason::NotAllowed,
            "The file has another name, which may be outside the folders the agent may send files from.",
        ));
    }
    let too_big = || {
        refused(
            Reason::NotSupported,
            "The file is larger than the agent may send (64 MiB).",
        )
    };
    if meta.len() > MAX_FILE {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unreadable())?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(too_big());
    }
    if bytes.is_empty() {
        return Err(refused(Reason::Invalid, "The file is empty."));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n.len() <= 255)
        .unwrap_or_else(|| "file".into());
    let media_type = media_type(&name).to_string();
    Ok(Outgoing {
        name,
        media_type,
        bytes,
    })
}

/// The media type for a file name, from its extension.
fn media_type(name: &str) -> &'static str {
    let extension = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "txt" | "log" | "text" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "json" => "application/json",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders(dir: &Path) -> Folders {
        Folders::new(&[dir.to_path_buf()], None)
    }

    fn reason(result: Result<Outgoing, Refusal>) -> Reason {
        result.expect_err("refused").reason
    }

    #[test]
    fn a_file_in_the_folder_is_read_with_its_name_and_type() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        let file = folders(dir.path())
            .read(&dir.path().join("notes.txt"))
            .unwrap();
        assert_eq!(file.name, "notes.txt");
        assert_eq!(file.media_type, "text/plain");
        assert_eq!(file.bytes, b"hello");
    }

    #[test]
    fn a_file_in_a_folder_inside_is_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("out")).unwrap();
        std::fs::write(dir.path().join("out/report.pdf"), "%PDF-").unwrap();
        let path = dir.path().join("out/../out/report.pdf");
        let file = folders(dir.path()).read(&path).unwrap();
        assert_eq!(file.media_type, "application/pdf");
    }

    #[test]
    fn a_file_outside_the_folders_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("secret.txt"), "key").unwrap();
        let out = folders(dir.path()).read(&other.path().join("secret.txt"));
        assert_eq!(reason(out), Reason::NotAllowed);
        // The same through `..`.
        let name = other.path().file_name().unwrap();
        let sneaky = dir.path().join("..").join(name).join("secret.txt");
        assert_eq!(
            reason(folders(dir.path()).read(&sneaky)),
            Reason::NotAllowed
        );
    }

    #[test]
    fn a_link_out_of_the_folder_is_refused_and_one_inside_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("secret.txt"), "key").unwrap();
        std::os::unix::fs::symlink(other.path().join("secret.txt"), dir.path().join("link.txt"))
            .unwrap();
        std::os::unix::fs::symlink(other.path(), dir.path().join("away")).unwrap();
        let f = folders(dir.path());
        assert_eq!(
            reason(f.read(&dir.path().join("link.txt"))),
            Reason::NotAllowed
        );
        assert_eq!(
            reason(f.read(&dir.path().join("away/secret.txt"))),
            Reason::NotAllowed
        );
        std::fs::write(dir.path().join("real.txt"), "fine").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("alias.md"))
            .unwrap();
        let file = f.read(&dir.path().join("alias.md")).unwrap();
        assert_eq!(file.name, "alias.md", "the name the agent gave");
        assert_eq!(file.bytes, b"fine");
    }

    #[test]
    fn a_relative_link_is_followed_inside_and_refused_when_it_climbs_out() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("secret.txt"), "key").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("real.txt"), "fine").unwrap();
        std::os::unix::fs::symlink("../real.txt", dir.path().join("sub/up.txt")).unwrap();
        let away = Path::new("../..")
            .join(other.path().strip_prefix("/").unwrap())
            .join("secret.txt");
        std::os::unix::fs::symlink(&away, dir.path().join("sub/out.txt")).unwrap();
        let f = folders(dir.path());
        assert_eq!(
            f.read(&dir.path().join("sub/up.txt")).unwrap().bytes,
            b"fine"
        );
        assert_eq!(
            reason(f.read(&dir.path().join("sub/out.txt"))),
            Reason::NotAllowed
        );
    }

    // The folder is named in the settings through a link: a path may name it either way.
    #[test]
    fn a_folder_reached_through_a_link_is_known_by_both_names() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir(base.path().join("real")).unwrap();
        std::fs::write(base.path().join("real/notes.txt"), "hello").unwrap();
        std::os::unix::fs::symlink(base.path().join("real"), base.path().join("named")).unwrap();
        let f = folders(&base.path().join("named"));
        assert!(f.read(&base.path().join("named/notes.txt")).is_ok());
        assert!(f.read(&base.path().join("real/notes.txt")).is_ok());
    }

    // A folder on the way that is swapped for a link to somewhere else while paths are followed
    // never leads out: each step starts from a folder already open, and a link is followed only
    // by its words, which here lead out and are refused.
    #[test]
    fn a_folder_on_the_way_swapped_for_a_link_never_leads_out() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("file.txt"), "secret").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/file.txt"), "inside").unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let swapper = {
            let (dir, other, stop) = (
                dir.path().to_path_buf(),
                other.path().to_path_buf(),
                stop.clone(),
            );
            std::thread::spawn(move || {
                let mut swaps = 0u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    std::fs::rename(dir.join("sub"), dir.join("parked")).unwrap();
                    std::os::unix::fs::symlink(&other, dir.join("sub")).unwrap();
                    std::thread::yield_now();
                    std::fs::remove_file(dir.join("sub")).unwrap();
                    std::fs::rename(dir.join("parked"), dir.join("sub")).unwrap();
                    swaps += 1;
                }
                swaps
            })
        };
        let f = folders(dir.path());
        let (mut sent, mut refused) = (0, 0);
        for _ in 0..20_000 {
            match f.read(&dir.path().join("sub/file.txt")) {
                Ok(file) => {
                    assert_eq!(file.bytes, b"inside", "a file from outside was read");
                    sent += 1;
                }
                Err(_) => refused += 1,
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let swaps = swapper.join().unwrap();
        assert!(swaps > 0 && sent + refused == 20_000);
    }

    // A folder on the way that cannot be opened is refused, never gone around.
    #[test]
    fn a_folder_on_the_way_that_cannot_be_opened_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("locked")).unwrap();
        std::fs::write(dir.path().join("locked/notes.txt"), "hello").unwrap();
        let locked = dir.path().join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&locked).is_ok(); // as root, nothing is locked
        let out = folders(dir.path()).read(&locked.join("notes.txt"));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        if !readable {
            assert_eq!(reason(out), Reason::Failed);
        }
    }

    // A folder the core opened stays the one it reads from: put back with a link in its place,
    // the link is not followed.
    #[test]
    fn the_folder_itself_swapped_for_a_link_is_not_followed() {
        let base = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("file.txt"), "secret").unwrap();
        std::fs::create_dir(base.path().join("work")).unwrap();
        let f = folders(&base.path().join("work"));
        std::fs::rename(base.path().join("work"), base.path().join("gone")).unwrap();
        std::os::unix::fs::symlink(other.path(), base.path().join("work")).unwrap();
        let out = f.read(&base.path().join("work/file.txt"));
        assert_eq!(
            reason(out),
            Reason::Invalid,
            "read from the folder held open"
        );
    }

    #[test]
    fn a_file_with_another_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("secret.txt"), "key").unwrap();
        if std::fs::hard_link(other.path().join("secret.txt"), dir.path().join("x.txt")).is_err() {
            return; // two folders on different file systems: nothing to test
        }
        let out = folders(dir.path()).read(&dir.path().join("x.txt"));
        assert_eq!(reason(out), Reason::NotAllowed);
    }

    #[test]
    fn what_is_not_an_ordinary_file_with_something_in_it_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let f = folders(dir.path());
        assert_eq!(reason(f.read(dir.path())), Reason::Invalid, "a folder");
        assert_eq!(reason(f.read(&dir.path().join("gone"))), Reason::Invalid);
        std::fs::write(dir.path().join("empty"), "").unwrap();
        assert_eq!(reason(f.read(&dir.path().join("empty"))), Reason::Invalid);
        assert_eq!(reason(f.read(Path::new("notes.txt"))), Reason::Invalid);
    }

    #[test]
    fn a_pipe_is_refused_without_waiting_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = dir.path().join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&pipe).status();
        if !made.is_ok_and(|s| s.success()) {
            return;
        }
        assert_eq!(reason(folders(dir.path()).read(&pipe)), Reason::Invalid);
    }

    #[test]
    fn a_file_over_the_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let big = std::fs::File::create(dir.path().join("big.bin")).unwrap();
        big.set_len(MAX_FILE + 1).unwrap();
        let out = folders(dir.path()).read(&dir.path().join("big.bin"));
        assert_eq!(reason(out), Reason::NotSupported);
    }

    #[test]
    fn no_listed_folder_is_holds_or_lies_inside_the_home() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let out = root.path().join("out");
        std::fs::create_dir_all(home.join("dev")).unwrap();
        std::fs::create_dir(&out).unwrap();
        std::os::unix::fs::symlink(&home, root.path().join("link")).unwrap();
        for bad in [
            home.clone(),
            home.join("dev"),
            root.path().to_path_buf(),
            root.path().join("link/dev"),
        ] {
            let why = apart_from_home(&[out.clone(), bad], &home).unwrap_err();
            assert!(!why.contains('/'), "{why}");
        }
        apart_from_home(&[out, root.path().join("missing")], &home).unwrap();
    }

    /// A working folder that holds the account home, as `~` holds an installed agent's: the home
    /// with its keys, and a file of the agent's work beside it.
    fn holding_the_home() -> (tempfile::TempDir, PathBuf) {
        let work = tempfile::tempdir().unwrap();
        let home = work.path().join("agent");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("keys.json"), "keys").unwrap();
        std::fs::write(work.path().join("notes.txt"), "work").unwrap();
        (work, home)
    }

    // The home holds the agent's keys: no file inside it is sent, wherever the allowed folder
    // is, and by whatever path or link the agent names it (D-099). The rest of the folder is.
    #[test]
    fn no_file_inside_the_account_home_is_sent_from_a_folder_that_holds_it() {
        let (work, home) = holding_the_home();
        let w = work.path();
        std::os::unix::fs::symlink(home.join("keys.json"), w.join("whole.json")).unwrap();
        std::os::unix::fs::symlink("agent/keys.json", w.join("relative.json")).unwrap();
        std::os::unix::fs::symlink(&home, w.join("home-link")).unwrap();
        std::fs::create_dir(w.join("out")).unwrap();
        std::os::unix::fs::symlink("../agent", w.join("out/up")).unwrap();
        let f = Folders::new(&[w.to_path_buf()], Some(&home));
        for path in [
            home.join("keys.json"),
            w.join("whole.json"),
            w.join("relative.json"),
            w.join("home-link/keys.json"),
            w.join("out/up/keys.json"),
            w.join("out/../agent/./keys.json"),
        ] {
            let refusal = f.read(&path).unwrap_err();
            assert_eq!(refusal.reason, Reason::NotAllowed, "{}", path.display());
            let detail = refusal.detail.unwrap();
            assert!(
                !detail.contains('/') && !detail.contains("agent/"),
                "{detail}"
            );
        }
        assert_eq!(f.read(&w.join("notes.txt")).unwrap().bytes, b"work");
        // A path that passes through the home and leaves it ends outside it.
        let back = f.read(&home.join("../notes.txt")).unwrap();
        assert_eq!(back.bytes, b"work");
    }

    // An allowed folder inside the home sends nothing from it, nor does the home itself.
    #[test]
    fn a_folder_inside_the_account_home_sends_nothing() {
        let (_work, home) = holding_the_home();
        std::fs::create_dir(home.join("out")).unwrap();
        std::fs::write(home.join("out/report.txt"), "report").unwrap();
        for folder in [home.clone(), home.join("out")] {
            let f = Folders::new(&[folder], Some(&home));
            let out = f.read(&home.join("out/report.txt"));
            assert_eq!(reason(out), Reason::NotAllowed);
        }
    }

    // Whether an allowed folder is in the home is asked of the folder held open, not of its name:
    // moved after it was opened, the folder keeps its own answer, whatever now has its name.
    #[test]
    fn whether_a_folder_is_in_the_home_follows_the_folder_held_open() {
        let (work, home) = holding_the_home();
        let home_id = id_of_path(&home).unwrap();
        let open = |path: &Path| File::open(path).unwrap();
        let inside = home.join("inside");
        let outside = work.path().join("outside");
        std::fs::create_dir(&inside).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let (was_inside, was_outside) = (open(&inside), open(&outside));
        assert!(lies_in(&open(&home), home_id));
        assert!(lies_in(&was_inside, home_id));
        assert!(!lies_in(&was_outside, home_id));

        // The two swap places: each keeps the answer of where it now is.
        let swap = work.path().join("swap");
        std::fs::rename(&inside, &swap).unwrap();
        std::fs::rename(&outside, &inside).unwrap();
        std::fs::rename(&swap, &outside).unwrap();
        assert!(!lies_in(&was_inside, home_id));
        assert!(lies_in(&was_outside, home_id));
    }

    fn settings(home: &Path, lines: &str) -> Config {
        let owner = "7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e";
        Config::parse(&format!(
            "home = {home:?}\nrelays = []\nowner = \"{owner}\"\n{lines}"
        ))
        .unwrap()
    }

    // The settings decide: by default the home is kept apart, from the harness's working folder
    // and from a listed folder alike, and one explicit line lets the owner allow it (D-099).
    #[test]
    fn the_owner_may_allow_files_from_the_account_home_with_one_line() {
        let (work, home) = holding_the_home();
        let keys = home.join("keys.json");
        let pi = format!("[pi]\ncwd = {:?}\n", work.path());
        let listed = format!("files_from = [{home:?}]\n[pi]\ncwd = \"/unused\"\n");
        let allow = "send_files_from_agent_home = true\n";

        let by_default = Folders::from_config(&settings(&home, &pi)).unwrap();
        assert_eq!(reason(by_default.read(&keys)), Reason::NotAllowed);
        assert!(by_default.read(&work.path().join("notes.txt")).is_ok());
        assert!(Folders::from_config(&settings(&home, &listed)).is_err());

        for lines in [format!("{allow}{pi}"), format!("{allow}{listed}")] {
            let allowed = Folders::from_config(&settings(&home, &lines)).unwrap();
            assert_eq!(allowed.read(&keys).unwrap().bytes, b"keys", "{lines}");
        }
    }

    #[test]
    fn with_no_folder_nothing_is_sent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        let none = Folders::new(&[dir.path().join("missing")], None);
        let out = none.read(&dir.path().join("notes.txt"));
        assert_eq!(reason(out), Reason::NotSupported);
    }

    // No refusal names the path, which may be logged or shown (D-071).
    #[test]
    fn a_refusal_names_no_path() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("secret.txt"), "key").unwrap();
        let refusal = folders(dir.path())
            .read(&other.path().join("secret.txt"))
            .unwrap_err();
        let detail = refusal.detail.unwrap();
        assert!(
            !detail.contains("secret") && !detail.contains('/'),
            "{detail}"
        );
    }
}
