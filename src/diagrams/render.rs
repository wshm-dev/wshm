//! Deterministic renderings of a validated [`Graph`]: the Mermaid flowchart
//! and the "Architecture" section of the PR analysis comment.

use super::graph::{clean_text, Graph};
use crate::config::{DiagramComment, DiagramText};

/// Fixed UI words of the comment section, per language (en fallback).
struct Words {
    architecture: &'static str,
    walkthrough: &'static str,
    steps: &'static str,
    components: &'static str,
    component: &'static str,
    file: &'static str,
    status: &'static str,
    role: &'static str,
    open_alt: &'static str,
    badge_label: &'static str,
    badge_message: &'static str,
    badge_message_diff: &'static str,
    new: &'static str,
    modified: &'static str,
    unchanged: &'static str,
}

const EN: Words = Words {
    architecture: "Architecture",
    walkthrough: "Walkthrough",
    steps: "steps",
    components: "Components",
    component: "Component",
    file: "File",
    status: "Status",
    role: "Role in this PR",
    open_alt: "Open the interactive architecture view",
    badge_label: "Open interactive view",
    badge_message: "architecture · walkthrough",
    badge_message_diff: "architecture · walkthrough · diff",
    new: "new",
    modified: "modified",
    unchanged: "unchanged",
};

const FR: Words = Words {
    architecture: "Architecture",
    walkthrough: "Parcours",
    steps: "étapes",
    components: "Composants",
    component: "Composant",
    file: "Fichier",
    status: "Statut",
    role: "Rôle dans cette PR",
    open_alt: "Ouvrir la vue interactive de l'architecture",
    badge_label: "Vue interactive",
    badge_message: "architecture · parcours",
    badge_message_diff: "architecture · parcours · diff",
    new: "nouveau",
    modified: "modifié",
    unchanged: "inchangé",
};

fn words(lang: &str) -> &'static Words {
    if lang.to_ascii_lowercase().starts_with("fr") {
        &FR
    } else {
        &EN
    }
}

/// Text inside a quoted Mermaid label. Strings are already clean, this is
/// defense in depth for graphs read back from the cache; `#` would start a
/// Mermaid entity code.
fn mm(s: &str) -> String {
    clean_text(s, 200).replace('#', "#35;")
}

/// Mermaid flowchart: one subgraph per layer (top to bottom), components
/// side by side, numbered edge labels, dashed edges for unchanged calls.
pub fn mermaid(g: &Graph) -> String {
    let mut out = String::from("flowchart TB\n");
    for (r, name) in g.layers.iter().enumerate() {
        let members: Vec<_> = g.nodes.iter().filter(|n| n.layer == r).collect();
        if members.is_empty() {
            continue;
        }
        let title = match mm(name) {
            t if t.is_empty() => " ".to_string(),
            t => t,
        };
        out.push_str(&format!("  subgraph L{r}[\"{title}\"]\n    direction LR\n"));
        for n in members {
            let file = if n.file.is_empty() {
                String::new()
            } else {
                format!("<br/><small>{}</small>", mm(&n.file))
            };
            out.push_str(&format!(
                "    n_{}[\"<b>{}</b>{file}\"]:::{}\n",
                n.id,
                mm(&n.label),
                status_class(&n.status)
            ));
        }
        out.push_str("  end\n");
    }
    for e in &g.edges {
        let arrow = if e.status == "unchanged" {
            "-.->"
        } else {
            "-->"
        };
        let label = mm(&e.label);
        let label = if label.is_empty() {
            e.step.to_string()
        } else {
            format!("{}. {label}", e.step)
        };
        out.push_str(&format!(
            "  n_{} {arrow}|\"{label}\"| n_{}\n",
            e.source, e.target
        ));
    }
    out.push_str(
        "  classDef new fill:#fdf0eb,stroke:#bf4520,stroke-width:2px,color:#2d3142\n\
         \x20 classDef modified fill:#ffffff,stroke:#2d3142,color:#2d3142\n\
         \x20 classDef unchanged fill:#f5f5f5,stroke:#8a93a6,stroke-dasharray:4 3,color:#4f5d75\n",
    );
    out
}

/// [`mermaid`] within `max_chars` (0 = no limit), degrading step by step:
/// edge labels dropped (step numbers kept), then unchanged nodes and their
/// edges, then both. `None` when even that is too long: the caller shows a
/// link instead.
pub fn mermaid_capped(g: &Graph, max_chars: usize) -> Option<String> {
    let fits = |m: &String| max_chars == 0 || m.chars().count() <= max_chars;
    let full = mermaid(g);
    if fits(&full) {
        return Some(full);
    }
    let mut no_labels = g.clone();
    for e in no_labels.edges.iter_mut() {
        e.label.clear();
    }
    let mut changed_only = g.clone();
    changed_only.nodes.retain(|n| n.is_changed());
    let kept: Vec<String> = changed_only.nodes.iter().map(|n| n.id.clone()).collect();
    changed_only
        .edges
        .retain(|e| kept.contains(&e.source) && kept.contains(&e.target));
    let mut both = changed_only.clone();
    for e in both.edges.iter_mut() {
        e.label.clear();
    }
    let mut candidates = vec![no_labels];
    if !changed_only.edges.is_empty() {
        candidates.push(changed_only);
        candidates.push(both);
    }
    candidates.iter().map(mermaid).find(fits)
}

fn status_class(s: &str) -> &'static str {
    match s {
        "new" => "new",
        "unchanged" => "unchanged",
        _ => "modified",
    }
}

/// Percent-encode for a shields.io path segment (`-` and `_` are doubled
/// first, as shields requires).
fn shields(s: &str) -> String {
    let s = s.replace('-', "--").replace('_', "__");
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Only plain http(s) URLs, with the characters that could end a Markdown
/// link destination percent-encoded.
fn safe_url(url: &str) -> Option<String> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let mut out = String::new();
    for c in url.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '"' => out.push_str("%22"),
            '`' => out.push_str("%60"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    Some(out)
}

fn badge(w: &Words, url: &str, with_diff: bool) -> Option<String> {
    let url = safe_url(url)?;
    let message = if with_diff {
        w.badge_message_diff
    } else {
        w.badge_message
    };
    Some(format!(
        "[![{}](https://img.shields.io/badge/{}-{}-bf4520?style=for-the-badge)]({url})",
        w.open_alt,
        shields(w.badge_label),
        shields(message),
    ))
}

/// Options of the comment section.
pub struct SectionOptions<'a> {
    pub language: &'a str,
    pub text: DiagramText,
    pub comment: DiagramComment,
    /// Link to the interactive page, when one was published.
    pub page_url: Option<&'a str>,
    /// The page embeds the diff.
    pub page_has_diff: bool,
    /// Size cap of the Mermaid block (chars, 0 = none), see [`mermaid_capped`].
    pub max_mermaid_chars: usize,
    /// The forge renders Mermaid in comments. When false the diagram is
    /// replaced by the page link (see `diagrams::forge_supports_mermaid`).
    pub supports_mermaid: bool,
}

/// The "Architecture" section inserted in the PR analysis comment (before
/// "### Review Checklist"). `None` when there is nothing to show.
pub fn comment_section(g: &Graph, o: &SectionOptions) -> Option<String> {
    let w = words(o.language);
    let badge = o.page_url.and_then(|u| badge(w, u, o.page_has_diff));
    let mut out = format!("### 🗺️ {}\n\n", w.architecture);

    if o.comment == DiagramComment::None {
        return badge.map(|b| format!("{out}{b}\n"));
    }

    if !g.story.is_empty() {
        out.push_str(&clean_text(&g.story, 1000));
        out.push_str("\n\n");
    }
    // Link-only fallback: forge without Mermaid, or a graph too big for the cap.
    let diagram = if o.supports_mermaid {
        mermaid_capped(g, o.max_mermaid_chars)
    } else {
        None
    };
    if let Some(m) = diagram {
        out.push_str("```mermaid\n");
        out.push_str(&m);
        out.push_str("```\n\n");
    }
    if let Some(b) = badge {
        out.push_str(&b);
        out.push_str("\n\n");
    }

    let label_of = |id: &str| {
        g.nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| clean_text(&n.label, 40))
            .unwrap_or_default()
    };
    out.push_str(&format!(
        "<details><summary><b>{}</b> · {} {}</summary>\n\n",
        w.walkthrough,
        g.edges.len(),
        w.steps
    ));
    for e in &g.edges {
        let label = clean_text(&e.label, 40);
        let label = if label.is_empty() {
            String::new()
        } else {
            format!(" (`{label}`)")
        };
        let caption = clean_text(&e.caption, 400);
        let caption = if caption.is_empty() {
            String::new()
        } else {
            format!(": {caption}")
        };
        out.push_str(&format!(
            "{}. **{} → {}**{label}{caption}\n",
            e.step,
            label_of(&e.source),
            label_of(&e.target)
        ));
    }
    out.push_str("\n</details>\n");

    if o.text != DiagramText::Short {
        out.push_str(&format!(
            "\n<details><summary><b>{}</b> · {}</summary>\n\n| {} | {} | {} | {} |\n|---|---|---|---|\n",
            w.components,
            g.nodes.len(),
            w.component,
            w.file,
            w.status,
            w.role
        ));
        for n in &g.nodes {
            let file = clean_text(&n.file, 120);
            let file = if file.is_empty() {
                "—".to_string()
            } else {
                format!("`{file}`")
            };
            let status = match n.status.as_str() {
                "new" => format!("🆕 {}", w.new),
                "unchanged" => format!("· {}", w.unchanged),
                _ => format!("✏️ {}", w.modified),
            };
            out.push_str(&format!(
                "| **{}** | {file} | {status} | {} |\n",
                clean_text(&n.label, 40),
                clean_text(&n.summary, 140)
            ));
        }
        out.push_str("\n</details>\n");
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagrams::graph::{normalize, Edge, Node};

    fn graph() -> Graph {
        Graph {
            title: "t".into(),
            story: "The story.".into(),
            layers: vec!["Commands".into(), "Core".into()],
            nodes: vec![
                Node {
                    id: "ls".into(),
                    label: "ls".into(),
                    file: "src/ls.rs".into(),
                    files: vec!["src/ls.rs".into()],
                    layer: 0,
                    status: "modified".into(),
                    summary: "Expands paths".into(),
                    detail: String::new(),
                },
                Node {
                    id: "glob".into(),
                    label: "operand_glob".into(),
                    file: "src/core/glob.rs".into(),
                    files: vec![],
                    layer: 1,
                    status: "new".into(),
                    summary: "Glob expander".into(),
                    detail: String::new(),
                },
                Node {
                    id: "end".into(),
                    label: "C#".into(),
                    file: String::new(),
                    files: vec![],
                    layer: 1,
                    status: "unchanged".into(),
                    summary: "Context".into(),
                    detail: String::new(),
                },
            ],
            edges: vec![
                Edge {
                    source: "ls".into(),
                    target: "glob".into(),
                    label: "free args".into(),
                    step: 1,
                    status: "modified".into(),
                    caption: "ls hands its paths over.".into(),
                },
                Edge {
                    source: "ls".into(),
                    target: "end".into(),
                    label: String::new(),
                    step: 2,
                    status: "unchanged".into(),
                    caption: String::new(),
                },
            ],
        }
    }

    #[test]
    fn mermaid_is_deterministic_and_well_formed() {
        let m = mermaid(&graph());
        assert_eq!(
            m,
            "flowchart TB\n\
             \x20 subgraph L0[\"Commands\"]\n    direction LR\n\
             \x20   n_ls[\"<b>ls</b><br/><small>src/ls.rs</small>\"]:::modified\n  end\n\
             \x20 subgraph L1[\"Core\"]\n    direction LR\n\
             \x20   n_glob[\"<b>operand_glob</b><br/><small>src/core/glob.rs</small>\"]:::new\n\
             \x20   n_end[\"<b>C#35;</b>\"]:::unchanged\n  end\n\
             \x20 n_ls -->|\"1. free args\"| n_glob\n\
             \x20 n_ls -.->|\"2\"| n_end\n\
             \x20 classDef new fill:#fdf0eb,stroke:#bf4520,stroke-width:2px,color:#2d3142\n\
             \x20 classDef modified fill:#ffffff,stroke:#2d3142,color:#2d3142\n\
             \x20 classDef unchanged fill:#f5f5f5,stroke:#8a93a6,stroke-dasharray:4 3,color:#4f5d75\n"
        );
    }

    #[test]
    fn mermaid_labels_cannot_break_out() {
        let raw = serde_json::json!({
            "layers": ["A\"]\nclick n_a href \"javascript:alert(1)\"", "B"],
            "nodes": [
                {"id": "a", "label": "x\"]\nclick a call evil()", "layer": 0},
                {"id": "b", "label": "y", "layer": 1}
            ],
            "edges": [{"source": "a", "target": "b", "label": "|\"x", "step": 1}]
        });
        let g = normalize(&raw, &[], DiagramText::Short).unwrap();
        let m = mermaid(&g);
        for line in m.lines() {
            let t = line.trim_start();
            assert!(!t.starts_with("click"), "directive injected: {line}");
            assert!(!t.contains("javascript:"), "{line}");
            // Quotes only as our own label delimiters.
            assert!(t.matches('"').count() % 2 == 0, "{line}");
        }
    }

    #[test]
    fn section_has_story_mermaid_badge_and_walkthrough() {
        let s = comment_section(
            &graph(),
            &SectionOptions {
                language: "en",
                text: DiagramText::Short,
                comment: DiagramComment::Mermaid,
                page_url: Some("https://x.example/p/a b?k=1"),
                page_has_diff: true,
                max_mermaid_chars: 5000,
                supports_mermaid: true,
            },
        )
        .unwrap();
        assert!(s.starts_with("### 🗺️ Architecture\n\nThe story.\n\n```mermaid\nflowchart TB\n"));
        assert!(s.contains(
            "[![Open the interactive architecture view](https://img.shields.io/badge/Open%20interactive%20view-architecture%20%C2%B7%20walkthrough%20%C2%B7%20diff-bf4520?style=for-the-badge)](https://x.example/p/a%20b?k=1)"
        ));
        assert!(s.contains("<details><summary><b>Walkthrough</b> · 2 steps</summary>"));
        assert!(s.contains("1. **ls → operand_glob** (`free args`): ls hands its paths over.\n"));
        assert!(s.contains("2. **ls → C#**\n"));
        assert!(
            !s.contains("Components"),
            "short text has no components table"
        );
    }

    #[test]
    fn section_normal_text_has_components_table_in_french() {
        let s = comment_section(
            &graph(),
            &SectionOptions {
                language: "fr",
                text: DiagramText::Normal,
                comment: DiagramComment::Mermaid,
                page_url: None,
                page_has_diff: false,
                max_mermaid_chars: 5000,
                supports_mermaid: true,
            },
        )
        .unwrap();
        assert!(!s.contains("shields.io"));
        assert!(s.contains("<b>Parcours</b> · 2 étapes"));
        assert!(s.contains("| Composant | Fichier | Statut | Rôle dans cette PR |"));
        assert!(
            s.contains("| **operand_glob** | `src/core/glob.rs` | 🆕 nouveau | Glob expander |")
        );
        assert!(s.contains("| **C#** | — | · inchangé | Context |"));
    }

    #[test]
    fn comment_none_is_link_only() {
        let opts = |url| SectionOptions {
            language: "en",
            text: DiagramText::Full,
            comment: DiagramComment::None,
            page_url: url,
            page_has_diff: false,
            max_mermaid_chars: 5000,
            supports_mermaid: true,
        };
        assert!(comment_section(&graph(), &opts(None)).is_none());
        let s = comment_section(&graph(), &opts(Some("https://e.x/p"))).unwrap();
        assert!(!s.contains("mermaid"));
        assert!(s.contains("](https://e.x/p)"));
        assert!(s.contains("architecture%20%C2%B7%20walkthrough-bf4520"));
    }

    #[test]
    fn non_http_links_are_dropped() {
        let s = comment_section(
            &graph(),
            &SectionOptions {
                language: "en",
                text: DiagramText::Short,
                comment: DiagramComment::Mermaid,
                page_url: Some("javascript:alert(1)"),
                page_has_diff: false,
                max_mermaid_chars: 5000,
                supports_mermaid: true,
            },
        )
        .unwrap();
        assert!(!s.contains("javascript"));
        assert!(!s.contains("shields.io"));
    }

    #[test]
    fn mermaid_cap_degrades_then_gives_up() {
        let g = graph();
        let full = mermaid(&g);
        let n = full.chars().count();
        assert_eq!(mermaid_capped(&g, 0).unwrap(), full);
        assert_eq!(mermaid_capped(&g, n).unwrap(), full);
        // One char short: edge labels go first, step numbers stay.
        let m = mermaid_capped(&g, n - 1).unwrap();
        assert!(!m.contains("free args") && m.contains("|\"1\"|"));
        assert!(m.contains("n_end"), "unchanged node still there");
        // Smaller: unchanged node and its edge dropped as well.
        let no_labels = m.chars().count();
        let m = mermaid_capped(&g, no_labels - 1).unwrap();
        assert!(!m.contains("n_end"));
        assert!(m.chars().count() < no_labels);
        assert!(mermaid_capped(&g, 50).is_none());
    }

    #[test]
    fn link_replaces_mermaid_when_unsupported_or_too_big() {
        for (supports, cap) in [(false, 5000), (true, 50)] {
            let s = comment_section(
                &graph(),
                &SectionOptions {
                    language: "en",
                    text: DiagramText::Short,
                    comment: DiagramComment::Mermaid,
                    page_url: Some("https://e.x/p"),
                    page_has_diff: false,
                    max_mermaid_chars: cap,
                    supports_mermaid: supports,
                },
            )
            .unwrap();
            assert!(!s.contains("```mermaid"));
            assert!(s.contains("](https://e.x/p)"));
            assert!(s.contains("The story."));
            assert!(s.contains("<b>Walkthrough</b>"));
        }
    }
}
