//! Where a chat's coding harness works (D-101).
//!
//! Each chat of a coding harness has a folder setting. A chat that chose none works in the
//! driver's own folder, the agent's install folder. Someone with the right to approve sets it from
//! the chat, by path, and it applies from the chat's next session. The owner lists in the settings
//! the folders agents may work in, by default the install folder, and a chat may pick any folder
//! inside one of them. Listing `~` allows any folder in the home.
//!
//! A path a chat gives is made whole: `~` stands for the home, and a relative path is taken from
//! the chat's current folder. It is then resolved, every link followed and every `..` taken away,
//! and must be a folder inside one the owner listed. The agent's account home, which holds its
//! keys, is never a chat's folder, nor is any folder inside it.
//!
//! A chat could learn which paths exist elsewhere on the machine from the reason it is given, so a
//! path is first looked at only as written: one that, `..` taken away, is not inside a listed
//! folder is refused as not allowed before anything on disk is read. Only then is it resolved, and
//! one that is not there, or is not a folder, is refused as invalid.
//!
//! Nothing here prints a path (D-071). A refusal says why in words the chat can read, and names no
//! path.

use marmot_agent_driver::{Reason, Refusal};
use std::path::{Component, Path, PathBuf};

/// The id of the setting, on a session item and in a `set` control.
pub const FOLDER: &str = "folder";

/// What a session item calls the setting.
pub const LABEL: &str = "Folder";

/// The rule for a chat's folder.
#[derive(Clone)]
pub struct FolderRule {
    /// The driver's own folder, used by a chat that chose none.
    default: PathBuf,
    /// The folders the owner listed, each by the names a path may reach it by: as written, made
    /// whole, and as its real path.
    roots: Vec<Vec<PathBuf>>,
    /// The account home, as its real path when it is there.
    account_home: PathBuf,
    /// The person's home, for a path that starts with `~`.
    home: Option<PathBuf>,
}

/// Shows how many folders are listed, and no path (D-071).
impl std::fmt::Debug for FolderRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FolderRule")
            .field("roots", &self.roots.len())
            .finish_non_exhaustive()
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
        "That folder is outside the folders the owner lets this agent work in.",
    )
}

fn keys() -> Refusal {
    refused(
        Reason::NotAllowed,
        "That folder holds the agent's own keys, so the agent may not work there.",
    )
}

fn missing() -> Refusal {
    refused(Reason::Invalid, "There is no such folder.")
}

/// `path` with every `.` and `..` taken away by its words alone, reading nothing on disk. A `..`
/// at the root stays at the root.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

impl FolderRule {
    /// The rule for a driver whose own folder is `default`, with the folders the owner `listed`,
    /// or else that folder alone, and the account home at `account_home`. A `~` at the start of a
    /// path a chat gives stands for the home in the `HOME` environment variable.
    pub fn new(default: &Path, listed: Option<&[PathBuf]>, account_home: &Path) -> Self {
        let roots = match listed {
            Some(listed) => listed.to_vec(),
            None => vec![default.to_path_buf()],
        };
        let roots = roots
            .iter()
            .map(|root| {
                let mut names = vec![lexical(root)];
                if let Ok(real) = std::fs::canonicalize(root)
                    && !names.contains(&real)
                {
                    names.push(real);
                }
                names
            })
            .collect();
        let account_home =
            std::fs::canonicalize(account_home).unwrap_or_else(|_| lexical(account_home));
        Self {
            default: default.to_path_buf(),
            roots,
            account_home,
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|h| h.is_absolute()),
        }
    }

    /// The driver's own folder, which a chat that chose none works in.
    pub fn default_folder(&self) -> &Path {
        &self.default
    }

    /// The folder a chat asked for, `asked`, taken from its current folder `current`: resolved,
    /// or why it may not be the chat's.
    pub fn check(&self, asked: &str, current: &Path) -> Result<PathBuf, Refusal> {
        let asked = asked.trim();
        if asked.is_empty() || asked.contains('\0') {
            return Err(missing());
        }
        let whole = if asked == "~" || asked.starts_with("~/") {
            let Some(home) = &self.home else {
                return Err(missing());
            };
            home.join(asked.trim_start_matches('~').trim_start_matches('/'))
        } else {
            current.join(asked)
        };
        if !whole.is_absolute() {
            return Err(missing());
        }
        // As written, before anything is read: a chat learns nothing of what lies outside.
        let written = lexical(&whole);
        let inside = |path: &Path| {
            (self.roots.iter()).any(|names| names.iter().any(|name| path.starts_with(name)))
        };
        if !inside(&written) {
            return Err(outside());
        }
        let real = std::fs::canonicalize(&whole).map_err(|_| missing())?;
        if !real.is_dir() {
            return Err(missing());
        }
        if !inside(&real) {
            return Err(outside());
        }
        if real.starts_with(&self.account_home) {
            return Err(keys());
        }
        Ok(real)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_taken_apart_by_its_words() {
        assert_eq!(lexical(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
        assert_eq!(lexical(Path::new("/../..")), PathBuf::from("/"));
    }

    // `~` is the home, and a folder in it is allowed when the home is listed.
    #[test]
    fn a_tilde_is_the_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(home.join("project")).unwrap();
        let mut rule =
            FolderRule::new(&home, Some(std::slice::from_ref(&home)), &home.join("keys"));
        rule.home = Some(home.clone());
        assert_eq!(
            rule.check("~/project", Path::new("/")).unwrap(),
            home.join("project")
        );
        assert_eq!(rule.check("~", Path::new("/")).unwrap(), home);
        rule.home = None;
        assert_eq!(
            rule.check("~/project", Path::new("/")).unwrap_err().reason,
            Reason::Invalid
        );
    }

    #[test]
    fn nothing_printed_shows_a_path() {
        let rule = FolderRule::new(Path::new("/home/alice/work"), None, Path::new("/x"));
        let printed = format!("{rule:?}");
        assert!(!printed.contains("alice"), "{printed}");
    }
}
