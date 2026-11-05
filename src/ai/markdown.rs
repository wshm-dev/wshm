//! Minimal Markdown segmentation: code (fenced blocks and inline code
//! spans) versus prose. Used so that hidden-content surfacing and posted-text
//! sanitising only rewrite prose and leave code (`Vec<String>`, `a<b`)
//! intact. Deliberately small: it follows the CommonMark rules that matter
//! here (fences of 3+ backticks or tildes, up to 3 spaces of indent, closed
//! by a fence of the same character at least as long; inline spans closed by
//! a backtick run of the same length) and treats anything unclosed as prose,
//! which is how the forge renders it.

/// One piece of a Markdown text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment<'a> {
    Prose(&'a str),
    /// Fenced block, including its fence lines.
    Fence(&'a str),
    /// Inline code span, including its backticks.
    Inline(&'a str),
}

impl<'a> Segment<'a> {
    pub fn text(&self) -> &'a str {
        match self {
            Segment::Prose(s) | Segment::Fence(s) | Segment::Inline(s) => s,
        }
    }

    pub fn is_code(&self) -> bool {
        !matches!(self, Segment::Prose(_))
    }
}

/// Fence opener of `line`: (char, run length), when it is one.
fn fence_of(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let c = rest.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = rest.chars().take_while(|&x| x == c).count();
    if n < 3 {
        return None;
    }
    // A backtick fence's info string cannot contain backticks.
    if c == '`' && rest[n..].contains('`') {
        return None;
    }
    Some((c, n))
}

/// Split `s` into prose, fenced blocks and inline code spans. Concatenating
/// the segments gives back `s` exactly.
pub fn segments(s: &str) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    // Pass 1: fenced blocks, line by line (keeping line endings).
    let mut prose_start = 0;
    let mut pos = 0;
    let mut open: Option<(char, usize, usize)> = None; // (char, len, block start)
    for line in s.split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let bare = line.trim_end_matches(['\n', '\r']);
        match open {
            None => {
                if let Some((c, n)) = fence_of(bare) {
                    if start > prose_start {
                        inline_segments(&s[prose_start..start], &mut out);
                    }
                    open = Some((c, n, start));
                }
            }
            Some((c, n, block_start)) => {
                let t = bare.trim_start_matches(' ');
                let indent = bare.len() - t.len();
                let run = t.chars().take_while(|&x| x == c).count();
                if indent <= 3 && run >= n && t[run..].trim().is_empty() {
                    out.push(Segment::Fence(&s[block_start..pos]));
                    prose_start = pos;
                    open = None;
                }
            }
        }
    }
    match open {
        // An unclosed fence runs to the end of the document.
        Some((_, _, block_start)) => {
            out.push(Segment::Fence(&s[block_start..]));
        }
        None if prose_start < s.len() => inline_segments(&s[prose_start..], &mut out),
        None => {}
    }
    out
}

fn inline_segments<'a>(s: &'a str, out: &mut Vec<Segment<'a>>) {
    let b = s.as_bytes();
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        if b[i] != b'`' {
            i += 1;
            continue;
        }
        let run_start = i;
        while i < b.len() && b[i] == b'`' {
            i += 1;
        }
        let n = i - run_start;
        // Find a closing run of exactly n backticks.
        let mut j = i;
        let mut close = None;
        while j < b.len() {
            if b[j] == b'`' {
                let k = j;
                while j < b.len() && b[j] == b'`' {
                    j += 1;
                }
                if j - k == n {
                    close = Some(j);
                    break;
                }
            } else {
                j += 1;
            }
        }
        if let Some(end) = close {
            if run_start > last {
                out.push(Segment::Prose(&s[last..run_start]));
            }
            out.push(Segment::Inline(&s[run_start..end]));
            last = end;
            i = end;
        }
        // Unclosed run: literal backticks, stays prose.
    }
    if last < s.len() {
        out.push(Segment::Prose(&s[last..]));
    }
}

/// Apply `f` to the prose segments only, keeping code verbatim.
pub fn map_prose(s: &str, mut f: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(s.len());
    for seg in segments(s) {
        match seg {
            Segment::Prose(p) => out.push_str(&f(p)),
            other => out.push_str(other.text()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_round_trip_and_classify() {
        let s = "a `Vec<T>` b\n```rust\nlet x = a<b;\n```\nc ``x ` y`` d `open";
        let segs = segments(s);
        let joined: String = segs.iter().map(|g| g.text()).collect();
        assert_eq!(joined, s);
        assert!(segs.contains(&Segment::Inline("`Vec<T>`")));
        assert!(segs.contains(&Segment::Fence("```rust\nlet x = a<b;\n```\n")));
        assert!(segs.contains(&Segment::Inline("``x ` y``")));
        assert!(matches!(segs.last(), Some(Segment::Prose(p)) if p.ends_with("`open")));
    }

    #[test]
    fn unclosed_fence_is_code_to_the_end() {
        let segs = segments("x\n~~~\n<b>hi</b>\n");
        assert_eq!(segs.last(), Some(&Segment::Fence("~~~\n<b>hi</b>\n")));
    }

    #[test]
    fn map_prose_leaves_code_alone() {
        let out = map_prose("<b> `<b>` <b>", |p| p.replace('<', "&lt;"));
        assert_eq!(out, "&lt;b> `<b>` &lt;b>");
    }
}
