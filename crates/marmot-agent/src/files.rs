//! Files the agent sends to a chat (D-090).
//!
//! A driver hands the core a path; the core reads the file only if it is in one of the folders the
//! settings allow, by default the harness's working folder. Every link in the path is followed
//! first, and what it leads to must still be inside: a link that leads out is refused, and so is a
//! file that also has a name somewhere else (a hard link), which could be outside. Only an
//! ordinary file is read, never a folder, a device or a pipe, and only up to [`MAX_FILE`].
//!
//! Nothing here prints a path, a name or a file's contents (D-071). A refusal says why in words
//! the chat can read, and names no path.

use marmot_agent_driver::{Reason, Refusal};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The largest file the agent may send: 64 MiB. MDK uploads a file it is handed in one piece,
/// held in memory, up to 512 MiB; this keeps well below that, and matches what today's Codex
/// connector takes in one batch.
pub const MAX_FILE: u64 = 64 * 1024 * 1024;

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

/// The folders the agent may send files from, with every link in them followed.
#[derive(Clone, Default)]
pub struct Folders {
    allowed: Vec<PathBuf>,
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

impl Folders {
    /// The folders, as their real paths. One that does not exist, or cannot be read, allows
    /// nothing.
    pub fn new(folders: &[PathBuf]) -> Self {
        let allowed = folders
            .iter()
            .filter_map(|folder| std::fs::canonicalize(folder).ok())
            .filter(|folder| folder.is_dir())
            .collect();
        Self { allowed }
    }

    /// Whether a real path is inside one of the folders.
    fn holds(&self, real: &Path) -> bool {
        self.allowed.iter().any(|folder| real.starts_with(folder))
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
        let real = std::fs::canonicalize(path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => refused(Reason::Invalid, "There is no such file."),
            _ => refused(Reason::Failed, "The file could not be read."),
        })?;
        if !self.holds(&real) {
            return Err(refused(
                Reason::NotAllowed,
                "The file is outside the folders the agent may send files from.",
            ));
        }
        let mut file = open(&real)?;
        let meta = file
            .metadata()
            .map_err(|_| refused(Reason::Failed, "The file could not be read."))?;
        if !meta.is_file() {
            return Err(refused(
                Reason::Invalid,
                "Only an ordinary file can be sent.",
            ));
        }
        if more_than_one_name(&meta) {
            return Err(refused(
                Reason::NotAllowed,
                "The file has another name, which may be outside the folders the agent may send files from.",
            ));
        }
        // What was opened is what the checks were about: a folder on the way swapped for a link
        // after the path was followed would have led somewhere else.
        if opened_elsewhere(&file, self) {
            return Err(refused(
                Reason::NotAllowed,
                "The file is outside the folders the agent may send files from.",
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
            .map_err(|_| refused(Reason::Failed, "The file could not be read."))?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(too_big());
        }
        if bytes.is_empty() {
            return Err(refused(Reason::Invalid, "The file is empty."));
        }
        let name = path
            .file_name()
            .or(real.file_name())
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
}

/// Opens a real path to read, without following a link at its end and without waiting on a pipe.
#[cfg(unix)]
fn open(real: &Path) -> Result<File, Refusal> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(real)
        .map_err(|error| match error.raw_os_error() {
            // The path was followed already, so a link here was put there since.
            Some(libc::ELOOP) => refused(
                Reason::NotAllowed,
                "The file is outside the folders the agent may send files from.",
            ),
            _ => refused(Reason::Failed, "The file could not be read."),
        })
}

#[cfg(not(unix))]
fn open(real: &Path) -> Result<File, Refusal> {
    File::open(real).map_err(|_| refused(Reason::Failed, "The file could not be read."))
}

#[cfg(unix)]
fn more_than_one_name(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() > 1
}

#[cfg(not(unix))]
fn more_than_one_name(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Whether the file that was opened is, by its real path now, outside the folders. Linux names
/// an open file's path; elsewhere this is not checked.
#[cfg(target_os = "linux")]
fn opened_elsewhere(file: &File, folders: &Folders) -> bool {
    use std::os::fd::AsRawFd;
    match std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())) {
        Ok(real) => !folders.holds(&real),
        Err(_) => false,
    }
}

#[cfg(not(target_os = "linux"))]
fn opened_elsewhere(_file: &File, _folders: &Folders) -> bool {
    false
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
        Folders::new(&[dir.to_path_buf()])
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(target_os = "linux")]
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
    fn with_no_folder_nothing_is_sent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        let none = Folders::new(&[dir.path().join("missing")]);
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
