//! The files a pull request changes, read from git. The head is the working tree, so uncommitted changes
//! and untracked files count; the base is the merge base of the given ref and HEAD, which is what a pull
//! request on GitHub is compared with.

use std::path::Path;
use std::process::Command;

use serde::Serialize;

pub struct Changes {
    pub base_ref: String,
    pub base_commit: String,
    pub head_commit: String,
    pub uncommitted: bool,
    /// Paths are relative to the project directory, and only files inside it are listed.
    pub files: Vec<ChangedFile>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl ChangedFile {
    pub fn base_path(&self) -> Option<&str> {
        match self.status {
            FileStatus::Added => None,
            FileStatus::Renamed => self.from.as_deref(),
            FileStatus::Modified | FileStatus::Deleted => Some(&self.path),
        }
    }

    pub fn head_path(&self) -> Option<&str> {
        match self.status {
            FileStatus::Deleted => None,
            _ => Some(&self.path),
        }
    }
}

impl Changes {
    pub fn read(project_dir: &Path, base: &str) -> Result<Changes, String> {
        let base_commit = git(project_dir, &["merge-base", base, "HEAD"]).map_err(|error| {
            format!("no merge base of {base} and HEAD ({error}); fetch the base branch first, with fetch-depth: 0 on actions/checkout")
        })?;
        let head_commit = git(project_dir, &["rev-parse", "HEAD"])?;
        let status = git(project_dir, &["status", "--porcelain", "--", "."])?;
        let listing = working_tree_diff(project_dir, &base_commit)?;

        let mut files = parse_name_status(&listing);
        files.sort();

        Ok(Changes { base_ref: base.to_string(), base_commit, head_commit, uncommitted: !status.is_empty(), files })
    }

    pub fn base_contents(&self, project_dir: &Path, path: &str) -> Result<Vec<u8>, String> {
        git_bytes(project_dir, &["show", &format!("{}:./{path}", self.base_commit)])
    }
}

/// `git diff --name-status -z` prints `M\0path\0`, and for a rename `R096\0old\0new\0`.
fn parse_name_status(listing: &str) -> Vec<ChangedFile> {
    let mut fields = listing.split('\0').filter(|field| !field.is_empty());
    let mut files = Vec::new();

    while let Some(status) = fields.next() {
        let Some(path) = fields.next() else {
            break;
        };

        let file = match status.chars().next() {
            Some('A') => ChangedFile { path: path.to_string(), status: FileStatus::Added, from: None },
            Some('D') => ChangedFile { path: path.to_string(), status: FileStatus::Deleted, from: None },
            Some('R') => {
                let Some(to) = fields.next() else {
                    break;
                };

                ChangedFile { path: to.to_string(), status: FileStatus::Renamed, from: Some(path.to_string()) }
            }
            _ => ChangedFile { path: path.to_string(), status: FileStatus::Modified, from: None },
        };

        files.push(file);
    }

    files
}

/// The working tree compared with a commit, untracked files included, so that git can tell a renamed
/// file from a deleted and an added one before anything is committed. Everything is staged into a copy
/// of the index, which leaves the real one alone.
fn working_tree_diff(dir: &Path, commit: &str) -> Result<String, String> {
    let index = git(dir, &["rev-parse", "--path-format=absolute", "--git-path", "index"])?;
    let copy = std::env::temp_dir().join(format!("puml-index-{}", std::process::id()));

    if Path::new(&index).exists() {
        std::fs::copy(&index, &copy).map_err(|error| format!("copying the git index: {error}"))?;
    }

    let with_copy = |arguments: &[&str]| {
        let mut command = Command::new("git");
        command.arg("-C").arg(dir).args(arguments).env("GIT_INDEX_FILE", &copy);

        run(command, arguments)
    };

    let listing = with_copy(&["add", "--all", "--", "."])
        .and_then(|_| with_copy(&["diff", "--cached", "--name-status", "-z", "-M", "--relative", commit, "--"]));

    let _ = std::fs::remove_file(&copy);

    String::from_utf8(listing?).map_err(|error| error.to_string())
}

fn git(dir: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = git_bytes(dir, arguments)?;

    String::from_utf8(output).map(|text| text.trim_end_matches('\n').to_string()).map_err(|error| error.to_string())
}

fn git_bytes(dir: &Path, arguments: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(arguments);

    run(command, arguments)
}

fn run(mut command: Command, arguments: &[&str]) -> Result<Vec<u8>, String> {
    let output = command.output().map_err(|error| format!("git: {error}"))?;

    if !output.status.success() {
        return Err(format!("git {}: {}", arguments.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }

    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_name_the_old_path_and_other_statuses_count_as_modified() {
        let files = parse_name_status("M\0app/A.php\0R087\0app/Old.php\0app/New.php\0A\0app/B.php\0T\0app/C.php\0D\0app/D.php\0");

        assert_eq!(
            files,
            vec![
                ChangedFile { path: "app/A.php".to_string(), status: FileStatus::Modified, from: None },
                ChangedFile { path: "app/New.php".to_string(), status: FileStatus::Renamed, from: Some("app/Old.php".to_string()) },
                ChangedFile { path: "app/B.php".to_string(), status: FileStatus::Added, from: None },
                ChangedFile { path: "app/C.php".to_string(), status: FileStatus::Modified, from: None },
                ChangedFile { path: "app/D.php".to_string(), status: FileStatus::Deleted, from: None },
            ]
        );
    }
}
