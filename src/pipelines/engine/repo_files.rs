//! Files of the repo's default branch, read from git objects of the local
//! checkout (`git show <rev>:<path>`), never from the working tree: the
//! checkout may be on another branch, stale, or hold untracked and
//! modified files. Only tracked files; `.wshm/` is never read.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Largest file read.
pub const MAX_FILE_BYTES: u64 = 1_000_000;

/// The default branch of a local checkout, pinned to one commit.
#[derive(Debug, Clone)]
pub struct Checkout {
    root: PathBuf,
    /// Commit id of the default branch.
    pub commit: String,
    /// The ref it was resolved from (e.g. `origin/main`).
    pub rev: String,
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Whether `rel` may be read: a clean relative path outside `.wshm/`.
fn allowed(rel: &str) -> Option<String> {
    let rel = crate::ai::output::sanitize_path(rel)?;
    if rel == ".wshm" || rel.starts_with(".wshm/") {
        return None;
    }
    Some(rel)
}

impl Checkout {
    /// The default branch of the checkout at `root`: `origin/HEAD`, else
    /// `origin/<base_branch>`, else the local `<base_branch>`. `None` when
    /// none exists (the checked-out branch is never used instead).
    pub fn open(root: &Path, base_branch: &str) -> Option<Self> {
        let base = base_branch.trim();
        let mut candidates = vec!["refs/remotes/origin/HEAD".to_string()];
        if !base.is_empty() && !base.starts_with('-') {
            candidates.push(format!("refs/remotes/origin/{base}"));
            candidates.push(format!("refs/heads/{base}"));
        }
        for rev in candidates {
            let spec = format!("{rev}^{{commit}}");
            if let Some(out) = git(root, &["rev-parse", "--verify", "--quiet", &spec]) {
                let commit = String::from_utf8_lossy(&out).trim().to_string();
                if !commit.is_empty() {
                    let rev = rev
                        .trim_start_matches("refs/remotes/")
                        .trim_start_matches("refs/heads/")
                        .to_string();
                    return Some(Self {
                        root: root.to_path_buf(),
                        commit,
                        rev,
                    });
                }
            }
        }
        None
    }

    /// A tracked file of the default branch, cut at `max` bytes. `None`
    /// when absent, not a file, too large, binary, or under `.wshm/`.
    pub fn read(&self, rel: &str, max: usize) -> Option<String> {
        let rel = allowed(rel)?;
        let obj = format!("{}:{rel}", self.commit);
        let kind = git(&self.root, &["cat-file", "-t", &obj])?;
        if String::from_utf8_lossy(&kind).trim() != "blob" {
            return None;
        }
        let size = git(&self.root, &["cat-file", "-s", &obj])?;
        let size: u64 = String::from_utf8_lossy(&size).trim().parse().ok()?;
        if size > MAX_FILE_BYTES {
            return None;
        }
        let bytes = git(&self.root, &["cat-file", "blob", &obj])?;
        let text = String::from_utf8(bytes).ok()?;
        let end = crate::ai::prompts::truncate_utf8(&text, max);
        Some(text[..end].to_string())
    }

    /// Tracked files of the default branch (sorted, `.wshm/` left out).
    pub fn files(&self) -> anyhow::Result<Vec<String>> {
        let out = git(
            &self.root,
            &["ls-tree", "-r", "--name-only", "-z", &self.commit],
        )
        .ok_or_else(|| anyhow::anyhow!("git ls-tree failed on {}", self.rev))?;
        let mut files: Vec<String> = String::from_utf8_lossy(&out)
            .split('\0')
            .filter_map(allowed)
            .collect();
        files.sort();
        files.dedup();
        Ok(files)
    }
}

/// Root of the local checkout of a repo config (`wshm_dir`'s parent).
pub fn checkout_root(config: &crate::config::Config) -> Option<PathBuf> {
    let root = config.wshm_dir.parent()?;
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    root.canonicalize().ok().filter(|p| p.is_dir())
}

/// The default-branch checkout of a repo config.
pub fn default_checkout(config: &crate::config::Config) -> Option<Checkout> {
    Checkout::open(&checkout_root(config)?, &config.fix.base_branch)
}

#[cfg(test)]
pub(crate) mod test_repo {
    use std::path::{Path, PathBuf};

    /// A temp git repo with `main` committed from `files`, then the working
    /// tree changed (`dirty`) and left uncommitted.
    pub fn make(files: &[(&str, &str)], dirty: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wshm-repo-{:x}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.canonicalize().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        let write = |p: &str, c: &str| {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, c).unwrap();
        };
        for (p, c) in files {
            write(p, c);
        }
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "init"]);
        for (p, c) in dirty {
            write(p, c);
        }
        root
    }

    pub fn remove(root: &Path) {
        std::fs::remove_dir_all(root).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_committed_default_branch_only() {
        let root = test_repo::make(
            &[
                ("CLAUDE.md", "committed"),
                (".wshm/config.toml", "x = 1"),
                ("src/a.rs", "fn a() {}\n"),
            ],
            &[
                ("CLAUDE.md", "modified in the working tree"),
                ("new.txt", "untracked"),
            ],
        );
        let c = Checkout::open(&root, "main").unwrap();
        assert_eq!(c.rev, "main");
        assert_eq!(c.read("CLAUDE.md", 100).as_deref(), Some("committed"));
        assert_eq!(c.read("CLAUDE.md", 3).as_deref(), Some("com"));
        assert!(c.read("new.txt", 100).is_none(), "untracked");
        assert!(c.read(".wshm/config.toml", 100).is_none());
        assert!(c.read("../etc/passwd", 100).is_none());
        assert!(c.read("src", 100).is_none(), "a tree is not a file");
        assert_eq!(c.files().unwrap(), ["CLAUDE.md", "src/a.rs"]);
        // Another branch checked out: still reads main.
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["checkout", "-q", "-b", "feature"])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(
            Checkout::open(&root, "develop").is_none(),
            "no fallback to the current branch"
        );
        test_repo::remove(&root);
    }
}
