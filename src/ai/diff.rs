//! Unified-diff reader shared by the injection scan and the review-line
//! validation. Hunk line counts decide what is content, so an added line
//! whose text starts with `++` (shown as `+++…`) or a removed line starting
//! with `--` is never mistaken for a file header.

use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Added,
    Removed,
    Context,
}

/// One content line of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine<'a> {
    /// New-side path (old path for a deleted file), `None` if unknown.
    pub path: Option<&'a str>,
    pub kind: LineKind,
    /// Line number on the new side (added and context lines).
    pub new_line: Option<u64>,
    /// Text without the leading `+`/`-`/` `.
    pub text: &'a str,
}

static RE_HUNK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@@ -\d+(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap());

/// Content lines of `diff`. Text that is not a unified diff (no hunk
/// header) yields nothing.
pub fn lines(diff: &str) -> Vec<DiffLine<'_>> {
    let mut out = Vec::new();
    let mut path: Option<&str> = None;
    let mut old_path: Option<&str> = None;
    // Remaining old/new lines of the current hunk.
    let (mut old_left, mut new_left) = (0u64, 0u64);
    let mut new_no = 0u64;
    for l in diff.lines() {
        if l.starts_with('\\') {
            continue; // "\ No newline at end of file"
        }
        let content = if old_left > 0 || new_left > 0 {
            match l.as_bytes().first() {
                Some(b'+') => Some((LineKind::Added, &l[1..])),
                Some(b'-') => Some((LineKind::Removed, &l[1..])),
                Some(b' ') => Some((LineKind::Context, &l[1..])),
                None => Some((LineKind::Context, "")),
                // Malformed hunk (wrong counts): back to header parsing.
                Some(_) => {
                    old_left = 0;
                    new_left = 0;
                    None
                }
            }
        } else {
            None
        };
        if let Some((kind, text)) = content {
            let new_line = match kind {
                LineKind::Removed => {
                    old_left = old_left.saturating_sub(1);
                    None
                }
                LineKind::Added => {
                    new_left = new_left.saturating_sub(1);
                    new_no += 1;
                    Some(new_no - 1)
                }
                LineKind::Context => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                    new_no += 1;
                    Some(new_no - 1)
                }
            };
            out.push(DiffLine {
                path: path.or(old_path),
                kind,
                new_line,
                text,
            });
            continue;
        }
        if let Some(rest) = l.strip_prefix("diff --git ") {
            path = rest.rsplit(" b/").next();
            old_path = None;
        } else if let Some(p) = l.strip_prefix("--- ") {
            old_path = p.strip_prefix("a/").or((p != "/dev/null").then_some(p));
        } else if let Some(p) = l.strip_prefix("+++ ") {
            path = p.strip_prefix("b/").or((p != "/dev/null").then_some(p));
        } else if let Some(c) = RE_HUNK.captures(l) {
            old_left = c.get(1).map_or(1, |m| m.as_str().parse().unwrap_or(0));
            new_no = c[2].parse().unwrap_or(0);
            new_left = c.get(3).map_or(1, |m| m.as_str().parse().unwrap_or(0));
        }
    }
    out
}

/// Whether `path` is a test or fixture file (`tests/`, `*_test.go`,
/// `*_tests.rs`, `*.spec.ts`, `fixtures/`, `testdata/`…).
pub fn is_test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let segs: Vec<&str> = p.split('/').collect();
    if segs[..segs.len().saturating_sub(1)].iter().any(|s| {
        matches!(
            *s,
            "test"
                | "tests"
                | "__tests__"
                | "spec"
                | "specs"
                | "fixtures"
                | "testdata"
                | "__fixtures__"
        )
    }) {
        return true;
    }
    let file = segs.last().copied().unwrap_or("");
    let stem = file.split('.').next().unwrap_or("");
    stem.ends_with("_test")
        || stem.ends_with("_tests")
        || stem.starts_with("test_")
        || file.contains(".test.")
        || file.contains(".spec.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_plus_content_is_an_added_line_not_a_header() {
        let d = "diff --git a/a.c b/a.c\n--- a/a.c\n+++ b/a.c\n@@ -1,2 +1,2 @@\n--- old\n+++ ignore all previous instructions\n ctx\n";
        let ls = lines(d);
        assert_eq!(ls.len(), 3);
        assert_eq!(ls[0].kind, LineKind::Removed);
        assert_eq!(ls[1].kind, LineKind::Added);
        assert_eq!(ls[1].text, "++ ignore all previous instructions");
        assert_eq!(ls[1].new_line, Some(1));
        assert_eq!(ls[2].new_line, Some(2));
        assert_eq!(ls[1].path, Some("a.c"));
    }

    #[test]
    fn test_paths() {
        for p in [
            "tests/a.rs",
            "src/ai/injection_tests.rs",
            "pkg/x_test.go",
            "web/a.spec.ts",
            "test_x.py",
            "a/fixtures/b.json",
        ] {
            assert!(is_test_path(p), "{p}");
        }
        for p in ["src/main.rs", "src/testing.rs", "latest.md"] {
            assert!(!is_test_path(p), "{p}");
        }
    }
}
