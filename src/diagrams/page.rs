//! Interactive page (React Flow viewer) rendered from a validated graph.
//!
//! The viewer's JS/CSS (React, React Flow, dagre, htm, diff2html, pinned in
//! `web/diagram-viewer`) are vendored into the binary: hosted pages load
//! them from `/diagrams/_assets/` under a strict CSP, static files inline
//! them. No CDN, no web font, no inline script: the page data is a
//! non-executable JSON block.

use super::graph::Graph;

const TEMPLATE: &str = include_str!("viewer.html");
/// Built by `web/diagram-viewer` (`bun run build`).
pub const VIEWER_JS: &str = include_str!("assets/viewer.js");
pub const VIEWER_CSS: &str = include_str!("assets/viewer.css");

/// CSP of hosted pages: only the vendored same-origin assets.
pub const HOSTED_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
     img-src 'self' data:; font-src 'self'; connect-src 'none'; base-uri 'none'; \
     form-action 'none'; frame-ancestors 'none'";

fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(s.as_bytes())[..6])
}

/// Versioned asset file names (content hash), served with an immutable cache.
pub fn asset_names() -> &'static (String, String) {
    static N: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    N.get_or_init(|| {
        (
            format!("viewer-{}.js", short_hash(VIEWER_JS)),
            format!("viewer-{}.css", short_hash(VIEWER_CSS)),
        )
    })
}

/// A vendored asset by its versioned file name: (content type, body).
pub fn asset(name: &str) -> Option<(&'static str, &'static str)> {
    let (js, css) = asset_names();
    if name == js {
        Some(("text/javascript; charset=utf-8", VIEWER_JS))
    } else if name == css {
        Some(("text/css; charset=utf-8", VIEWER_CSS))
    } else {
        None
    }
}

/// JSON safe inside a `<script>` element: `<` is escaped so neither
/// `</script>` nor `<!--` can appear (still valid JSON).
fn script_json(v: &impl serde::Serialize) -> String {
    serde_json::to_string(v)
        .unwrap_or_else(|_| "null".into())
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Language attribute: letters, digits and `-` only.
fn lang_attr(lang: &str) -> String {
    let l: String = lang
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(12)
        .collect();
    if l.is_empty() {
        "en".into()
    } else {
        l
    }
}

/// Fill `{{wshm:name}}` markers in ONE pass over the template: substituted
/// values are never scanned again, so a value containing a marker (e.g. a
/// hostile title) stays plain data.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out =
        String::with_capacity(template.len() + values.iter().map(|v| v.1.len()).sum::<usize>());
    let mut rest = template;
    while let Some(i) = rest.find("{{wshm:") {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        match values
            .iter()
            .find(|(name, _)| after.starts_with(&format!("{{{{wshm:{name}}}}}")))
        {
            Some((name, value)) => {
                out.push_str(value);
                rest = &after[format!("{{{{wshm:{name}}}}}").len()..];
            }
            None => {
                out.push_str("{{wshm:");
                rest = &after["{{wshm:".len()..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Fingerprint of the compiled-in viewer (part of ETags and page hashes).
pub fn template_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| short_hash(&format!("{TEMPLATE}{VIEWER_JS}{VIEWER_CSS}")))
}

/// The diff to embed: only with `diff_viewer` on and within `max_diff_kb`.
/// Bigger diffs are left out and the viewer links to "Files changed".
pub fn embeddable_diff(diff: &str, diff_viewer: bool, max_diff_kb: u32) -> Option<&str> {
    (diff_viewer && !diff.is_empty() && diff.len() <= max_diff_kb as usize * 1024).then_some(diff)
}

/// How the vendored assets are delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assets {
    /// Same-origin `/diagrams/_assets/...` (daemon-hosted page, strict CSP).
    Hosted,
    /// Inlined: a self-contained file (local, GitHub Pages, object stores).
    Inline,
}

pub struct PageOptions<'a> {
    pub language: &'a str,
    pub repo: &'a str,
    pub pr: u64,
    /// Raw PR diff (see [`embeddable_diff`]); `None` = not embedded.
    pub diff: Option<&'a str>,
    /// Link shown instead of the diff drawer when the diff is not embedded
    /// (too large, or `diff_viewer` off).
    pub files_url: Option<&'a str>,
    pub assets: Assets,
}

/// Render the HTML page.
pub fn render(g: &Graph, o: &PageOptions) -> String {
    // The viewer ships English and French UI strings.
    let ui_lang = if o.language.to_ascii_lowercase().starts_with("fr") {
        "fr"
    } else {
        "en"
    };
    let eyebrow = if ui_lang == "fr" {
        format!("Architecture · {} #{} · générée par wshm", o.repo, o.pr)
    } else {
        format!("Architecture · {} #{} · generated by wshm", o.repo, o.pr)
    };
    let data = serde_json::json!({
        "graph": g,
        "meta": {
            "lang": ui_lang,
            "repo": o.repo,
            "pr": o.pr,
            "diff": o.diff.is_some(),
            "files_url": o.files_url,
            "eyebrow": eyebrow,
        },
        "diff": o.diff.unwrap_or(""),
    });
    let title = if g.title.is_empty() {
        format!("{} #{}", o.repo, o.pr)
    } else {
        format!("{} · {} #{}", g.title, o.repo, o.pr)
    };
    let (style, script) = match o.assets {
        Assets::Hosted => {
            let (js, css) = asset_names();
            (
                format!("<link rel=\"stylesheet\" href=\"/diagrams/_assets/{css}\">"),
                format!("<script src=\"/diagrams/_assets/{js}\"></script>"),
            )
        }
        Assets::Inline => (
            format!(
                "<style>{}</style>",
                VIEWER_CSS.replace("</style", "<\\/style")
            ),
            format!(
                "<script>{}</script>",
                VIEWER_JS.replace("</script", "<\\/script")
            ),
        ),
    };
    fill(
        TEMPLATE,
        &[
            ("lang", &lang_attr(o.language)),
            ("title", &html_escape(&title)),
            ("style", &style),
            ("data", &script_json(&data)),
            ("script", &script),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(assets: Assets) -> PageOptions<'static> {
        PageOptions {
            language: "en",
            repo: "o/r",
            pr: 1,
            diff: None,
            files_url: Some("https://github.com/o/r/pull/1/files"),
            assets,
        }
    }

    /// The JSON data block of a rendered page.
    fn data_of(html: &str) -> serde_json::Value {
        let start = html.find("id=\"wshm-data\">").unwrap() + "id=\"wshm-data\">".len();
        let end = html[start..].find("</script>").unwrap() + start;
        serde_json::from_str(&html[start..end]).unwrap()
    }

    #[test]
    fn template_has_the_markers() {
        for m in ["lang", "title", "style", "data", "script"] {
            assert_eq!(
                TEMPLATE.matches(&format!("{{{{wshm:{m}}}}}")).count(),
                1,
                "{m}"
            );
        }
    }

    #[test]
    fn values_cannot_close_the_script_block() {
        let g = Graph {
            title: "x </script><script>alert(1)</script>".into(),
            ..Default::default()
        };
        let diff = "+ let s = \"</script><!--\";\n";
        let html = render(
            &g,
            &PageOptions {
                language: "fr\"><script>",
                diff: Some(diff),
                files_url: None,
                ..opts(Assets::Hosted)
            },
        );
        // Our two script elements only (data block + vendored bundle).
        assert_eq!(html.matches("</script>").count(), 2);
        assert!(!html.contains("<!--\""));
        assert!(html.contains("<html lang=\"frscript\">"));
        let data = data_of(&html);
        assert_eq!(data["diff"], diff, "the diff round-trips");
        assert_eq!(data["meta"]["diff"], true);
        assert_eq!(data["meta"]["lang"], "fr");
    }

    #[test]
    fn hostile_title_with_markers_stays_data() {
        // Titles are model output: markers (old and new style) must not be
        // expanded or break the page.
        let g = Graph {
            title: "__GRAPH__ __META__ __DIFF__ {{wshm:data}} {{wshm:script}} {{wshm:".into(),
            ..Default::default()
        };
        let html = render(&g, &opts(Assets::Hosted));
        let data = data_of(&html);
        assert_eq!(data["graph"]["title"], g.title.as_str());
        assert_eq!(html.matches("id=\"wshm-data\"").count(), 1);
        assert!(html.contains("<title>__GRAPH__ __META__ __DIFF__ {{wshm:data}} {{wshm:script}} {{wshm: · o/r #1</title>"));
        assert_eq!(html.matches("<script src=").count(), 1);
    }

    #[test]
    fn hosted_pages_use_same_origin_assets_and_no_cdn() {
        let html = render(&Graph::default(), &opts(Assets::Hosted));
        let (js, css) = asset_names();
        assert!(html.contains(&format!("<script src=\"/diagrams/_assets/{js}\"></script>")));
        assert!(html.contains(&format!("href=\"/diagrams/_assets/{css}\"")));
        assert!(
            !html.contains("https://cdn") && !html.contains("esm.sh") && !html.contains("fonts.g")
        );
        assert!(html.len() < 4096, "assets are not inlined");
        assert_eq!(asset(js).unwrap().1, VIEWER_JS);
        assert!(asset("viewer.js").is_none(), "only versioned names");
        let data = data_of(&html);
        assert_eq!(data["meta"]["diff"], false);
        assert_eq!(
            data["meta"]["files_url"],
            "https://github.com/o/r/pull/1/files"
        );
    }

    #[test]
    fn static_pages_are_self_contained() {
        let html = render(&Graph::default(), &opts(Assets::Inline));
        assert!(!html.contains("/diagrams/_assets/"));
        assert!(
            !html.contains("esm.sh")
                && !html.contains("cdn.jsdelivr")
                && !html.contains("fonts.googleapis")
        );
        assert!(html.len() > VIEWER_JS.len());
        // The inlined bundle cannot end the script element early.
        let script_start = html.rfind("<script>").unwrap();
        assert_eq!(html[script_start..].matches("</script").count(), 1);
    }

    #[test]
    fn vendored_bundle_has_no_remote_imports() {
        assert!(!VIEWER_JS.contains("esm.sh") && !VIEWER_JS.contains("cdn.jsdelivr"));
        assert!(!VIEWER_CSS.contains("@import") && !VIEWER_CSS.contains("fonts.googleapis"));
    }

    #[test]
    fn diff_cap() {
        let d = "x".repeat(2048);
        assert_eq!(embeddable_diff(&d, true, 2), Some(d.as_str()));
        assert_eq!(embeddable_diff(&d, true, 1), None, "over max_diff_kb");
        assert_eq!(embeddable_diff(&d, false, 10), None, "diff_viewer off");
        assert_eq!(embeddable_diff("", true, 10), None);
    }
}
