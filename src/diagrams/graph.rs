//! Architecture graph produced by the AI, and its validation.
//!
//! The model output is untrusted (it is derived from the PR content, which
//! anyone can write): every string is sanitized for Markdown and Mermaid, the
//! structure is repaired (unique ids, dangling edges, layering, caps) and the
//! file lists are restricted to paths really present in the diff.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::DiagramText;

pub const MAX_NODES: usize = 9;
pub const MAX_EDGES: usize = 12;
const MAX_FILES_PER_NODE: usize = 20;
const MAX_LAYER_NAMES_PER_ROW: usize = 2;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Graph {
    pub title: String,
    pub story: String,
    /// One name per row, top to bottom (index = `Node::layer`).
    pub layers: Vec<String>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: String,
    pub label: String,
    /// Main file, for display (may be outside the diff for "unchanged").
    pub file: String,
    /// Paths of the diff that belong to this component (exact diff paths).
    #[serde(default)]
    pub files: Vec<String>,
    pub layer: usize,
    /// "new" | "modified" | "unchanged".
    pub status: String,
    pub summary: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub label: String,
    pub step: u32,
    pub status: String,
    pub caption: String,
}

impl Node {
    pub fn is_changed(&self) -> bool {
        self.status != "unchanged"
    }
}

/// Make a model string safe to embed in Markdown, a Markdown table cell and
/// a quoted Mermaid label: single line, no backticks, double quotes, pipes,
/// angle brackets (HTML), links/images, mentions, bidi or control characters;
/// truncated to `max` characters on a word boundary.
pub fn clean_text(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '`' => {}
            '"' => out.push('\''),
            '|' => out.push('/'),
            '<' => out.push('‹'),
            '>' => out.push('›'),
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => {}
            '\u{FEFF}' => {}
            // A mention would ping a GitHub user from a bot comment.
            '@' => out.push_str("@\u{200B}"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    let mut out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // Inline links and images need "](": break it.
    while out.contains("](") {
        out = out.replace("](", "] (");
    }
    loop {
        let lower = out.to_ascii_lowercase();
        let Some(i) = lower.find("javascript:") else {
            break;
        };
        out.replace_range(i..i + "javascript:".len(), "");
    }
    let out = out
        .trim_start_matches(|c: char| "#-+=*~_ ".contains(c))
        .trim()
        .to_string();
    truncate_words(&out, max)
}

/// Truncate to at most `max` chars, on a word boundary, with an ellipsis.
fn truncate_words(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    let cut = match cut.rfind(' ') {
        Some(i) if i > cut.len() / 2 => &cut[..i],
        _ => cut.as_str(),
    };
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.', ' ']))
}

/// Keep at most `n` words.
fn cap_words(s: &str, n: usize) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() <= n {
        return s.to_string();
    }
    format!(
        "{}…",
        words[..n].join(" ").trim_end_matches([',', ';', ':', '.'])
    )
}

/// Node id usable as a Mermaid id and a JSON key: `[a-z0-9_]`, ≤ 32 chars.
fn clean_id(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').chars().take(32).collect()
}

fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn num_of(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn status_of(v: &Value) -> Option<&'static str> {
    match str_of(v).trim().to_ascii_lowercase().as_str() {
        "new" | "added" | "add" => Some("new"),
        "modified" | "changed" | "updated" | "mod" => Some("modified"),
        "unchanged" | "context" | "existing" => Some("unchanged"),
        _ => None,
    }
}

/// Map model-given paths onto paths of the diff: exact match, then a unique
/// suffix match (`ls.rs` → `src/cmds/ls.rs`), then a directory prefix.
fn resolve_files(cands: &[String], diff_paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |p: &String, out: &mut Vec<String>| {
        if !out.contains(p) && out.len() < MAX_FILES_PER_NODE {
            out.push(p.clone());
        }
    };
    for c in cands {
        let c = c.trim().trim_start_matches("./");
        if c.is_empty() {
            continue;
        }
        if let Some(p) = diff_paths.iter().find(|p| p.as_str() == c) {
            push(p, &mut out);
            continue;
        }
        let suffix = format!("/{}", c.trim_start_matches('/'));
        let by_suffix: Vec<&String> = diff_paths.iter().filter(|p| p.ends_with(&suffix)).collect();
        if by_suffix.len() == 1 {
            push(by_suffix[0], &mut out);
            continue;
        }
        let dir = format!("{}/", c.trim_end_matches('/'));
        for p in diff_paths.iter().filter(|p| p.starts_with(&dir)) {
            push(p, &mut out);
        }
    }
    out
}

/// Character budgets per text level.
struct Budget {
    story: usize,
    summary: usize,
    detail: usize,
    caption: usize,
    caption_words: usize,
}

fn budget(text: DiagramText) -> Budget {
    match text {
        DiagramText::Short => Budget {
            story: 300,
            summary: 100,
            detail: 0,
            caption: 90,
            caption_words: 12,
        },
        DiagramText::Normal => Budget {
            story: 500,
            summary: 120,
            detail: 400,
            caption: 200,
            caption_words: 40,
        },
        DiagramText::Full => Budget {
            story: 800,
            summary: 140,
            detail: 700,
            caption: 400,
            caption_words: 80,
        },
    }
}

/// Validate and repair the model output. Returns `None` when nothing worth
/// drawing is left (fewer than 2 connected nodes).
pub fn normalize(raw: &Value, diff_paths: &[String], text: DiagramText) -> Option<Graph> {
    let b = budget(text);
    let raw_layers: Vec<String> = raw
        .get("layers")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| clean_text(&str_of(v), 40)).collect())
        .unwrap_or_default();
    let raw_nodes = raw.get("nodes").and_then(Value::as_array)?;

    // ── Nodes: unique ids, clean texts, files restricted to the diff ──
    let mut nodes: Vec<Node> = Vec::new();
    // Original layer name of each node (to name the rows after relayout).
    let mut layer_name: HashMap<String, String> = HashMap::new();
    let mut id_map: HashMap<String, String> = HashMap::new();
    for (i, n) in raw_nodes.iter().enumerate() {
        let raw_id = str_of(n.get("id").unwrap_or(&Value::Null));
        if id_map.contains_key(&raw_id) {
            continue; // same id twice: a duplicate node, edges can't tell them apart
        }
        let mut id = clean_id(&raw_id);
        if id.is_empty() {
            id = format!("n{i}");
        }
        let base = id.clone();
        let mut k = 2;
        while nodes.iter().any(|x| x.id == id) {
            id = format!("{base}_{k}");
            k += 1;
        }
        id_map.insert(raw_id.clone(), id.clone());

        let get = |k: &str| str_of(n.get(k).unwrap_or(&Value::Null));
        let file = clean_text(&get("file"), 120);
        let mut cands: Vec<String> = n
            .get("files")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(str_of).collect())
            .unwrap_or_default();
        cands.push(get("file"));
        let files = resolve_files(&cands, diff_paths);
        let status = status_of(n.get("status").unwrap_or(&Value::Null))
            .unwrap_or(if files.is_empty() {
                "unchanged"
            } else {
                "modified"
            })
            .to_string();
        let layer = match n.get("layer") {
            Some(Value::String(s)) if s.trim().parse::<u64>().is_err() => raw_layers
                .iter()
                .position(|l| l.eq_ignore_ascii_case(&clean_text(s, 40)))
                .unwrap_or(0),
            Some(v) => num_of(v).unwrap_or(0) as usize,
            None => 0,
        };
        let layer = layer.min(raw_layers.len().saturating_sub(1));
        layer_name.insert(
            id.clone(),
            raw_layers.get(layer).cloned().unwrap_or_default(),
        );
        let label = match clean_text(&get("label"), 40) {
            l if l.is_empty() => clean_text(&raw_id, 40),
            l => l,
        };
        nodes.push(Node {
            id,
            label,
            file,
            files,
            layer,
            status,
            summary: clean_text(&get("summary"), b.summary),
            detail: if b.detail == 0 {
                String::new()
            } else {
                clean_text(&get("detail"), b.detail)
            },
        });
    }

    // ── Edges: existing endpoints, no self-loop, no duplicate ──
    let mut edges: Vec<(u64, usize, Edge)> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    if let Some(raw_edges) = raw.get("edges").and_then(Value::as_array) {
        for (i, e) in raw_edges.iter().enumerate() {
            let get = |k: &str| str_of(e.get(k).unwrap_or(&Value::Null));
            let (Some(s), Some(t)) = (id_map.get(&get("source")), id_map.get(&get("target")))
            else {
                continue;
            };
            if s == t || !seen.insert((s.clone(), t.clone())) {
                continue;
            }
            let step = e.get("step").and_then(num_of).unwrap_or(1_000 + i as u64);
            let caption = cap_words(&clean_text(&get("caption"), b.caption), b.caption_words);
            edges.push((
                step,
                i,
                Edge {
                    source: s.clone(),
                    target: t.clone(),
                    label: cap_words(&clean_text(&get("label"), 40), 5),
                    step: 0,
                    status: status_of(e.get("status").unwrap_or(&Value::Null))
                        .unwrap_or("modified")
                        .to_string(),
                    caption,
                },
            ));
        }
    }
    edges.sort_by_key(|(step, i, _)| (*step, *i));
    let mut edges: Vec<Edge> = edges.into_iter().map(|(_, _, e)| e).collect();

    // ── Cap nodes: keep the endpoints of the first edges in reading order, so
    // the kept nodes stay connected (isolated nodes are dropped below anyway) ──
    if nodes.len() > MAX_NODES {
        let mut keep: Vec<&str> = Vec::new();
        for e in &edges {
            let new: Vec<&str> = [e.source.as_str(), e.target.as_str()]
                .into_iter()
                .filter(|id| !keep.contains(id))
                .collect();
            if keep.len() + new.len() <= MAX_NODES {
                keep.extend(new);
            }
        }
        let keep: HashSet<String> = keep.into_iter().map(String::from).collect();
        nodes.retain(|n| keep.contains(&n.id));
    }
    let ids: HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
    edges.retain(|e| ids.contains(&e.source) && ids.contains(&e.target));

    // ── Break cycles (keep edges in reading order), then cap ──
    let mut dag: Vec<Edge> = Vec::new();
    for e in edges {
        if dag.len() >= MAX_EDGES {
            break;
        }
        if !reaches(&dag, &e.target, &e.source) {
            dag.push(e);
        }
    }
    let mut edges = dag;
    for (i, e) in edges.iter_mut().enumerate() {
        e.step = i as u32 + 1;
    }

    // ── Drop isolated nodes ──
    let connected: HashSet<&str> = edges
        .iter()
        .flat_map(|e| [e.source.as_str(), e.target.as_str()])
        .collect();
    nodes.retain(|n| connected.contains(n.id.as_str()));
    if nodes.len() < 2 || edges.is_empty() {
        return None;
    }

    // ── Layering: every callee strictly below its caller ──
    let mut row: HashMap<String, usize> = nodes.iter().map(|n| (n.id.clone(), n.layer)).collect();
    for id in topo_order(&nodes, &edges) {
        let r = row[&id];
        for e in edges.iter().filter(|e| e.source == id) {
            let t = row.get_mut(&e.target).expect("edge endpoints exist");
            if *t <= r {
                *t = r + 1;
            }
        }
    }
    // Compact empty rows.
    let mut used: Vec<usize> = row.values().copied().collect();
    used.sort_unstable();
    used.dedup();
    let compact: HashMap<usize, usize> = used.iter().enumerate().map(|(i, r)| (*r, i)).collect();
    let mut layers: Vec<Vec<String>> = vec![Vec::new(); used.len()];
    for n in nodes.iter_mut() {
        n.layer = compact[&row[&n.id]];
        let name = layer_name.get(&n.id).cloned().unwrap_or_default();
        let names = &mut layers[n.layer];
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    let layers = layers
        .into_iter()
        .map(|names| {
            names
                .into_iter()
                .take(MAX_LAYER_NAMES_PER_ROW)
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .collect();

    Some(Graph {
        title: clean_text(&str_of(raw.get("title").unwrap_or(&Value::Null)), 100),
        story: clean_text(&str_of(raw.get("story").unwrap_or(&Value::Null)), b.story),
        layers,
        nodes,
        edges,
    })
}

/// Is `to` reachable from `from` along `edges`?
fn reaches(edges: &[Edge], from: &str, to: &str) -> bool {
    let mut stack = vec![from];
    let mut seen: HashSet<&str> = HashSet::new();
    while let Some(n) = stack.pop() {
        if n == to {
            return true;
        }
        if !seen.insert(n) {
            continue;
        }
        stack.extend(
            edges
                .iter()
                .filter(|e| e.source == n)
                .map(|e| e.target.as_str()),
        );
    }
    false
}

/// Kahn order of an acyclic graph (node order breaks ties).
fn topo_order(nodes: &[Node], edges: &[Edge]) -> Vec<String> {
    let mut indeg: HashMap<&str, usize> = nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
    for e in edges {
        *indeg
            .get_mut(e.target.as_str())
            .expect("edge endpoints exist") += 1;
    }
    let mut out = Vec::with_capacity(nodes.len());
    let mut ready: Vec<&str> = nodes
        .iter()
        .map(|n| n.id.as_str())
        .filter(|id| indeg[id] == 0)
        .collect();
    while !ready.is_empty() {
        let id = ready.remove(0);
        out.push(id.to_string());
        for e in edges.iter().filter(|e| e.source == id) {
            let d = indeg
                .get_mut(e.target.as_str())
                .expect("edge endpoints exist");
            *d -= 1;
            if *d == 0 {
                ready.push(e.target.as_str());
            }
        }
    }
    out
}

/// Paths of a unified diff, from its `diff --git a/x b/y` lines.
pub fn diff_paths(diff: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(p) = rest.rsplit(" b/").next() {
                let p = p.trim().to_string();
                if !p.is_empty() && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paths() -> Vec<String> {
        vec![
            "src/cmds/ls.rs".into(),
            "src/core/glob.rs".into(),
            "src/core/glob/tests.rs".into(),
        ]
    }

    #[test]
    fn clean_text_neutralizes_markdown_mermaid_and_html() {
        let s = clean_text(
            "## Use `rm` <script>alert(1)</script> | \"x\" [a](http://e) @bob javascript:x\nnext",
            200,
        );
        assert!(!s.contains('`'));
        assert!(!s.contains('<') && !s.contains('>'));
        assert!(!s.contains('|') && !s.contains('"'));
        assert!(!s.contains("]("));
        assert!(!s.contains('\n'));
        assert!(!s.to_ascii_lowercase().contains("javascript:"));
        assert!(!s.contains("@bob"), "mentions are broken: {s}");
        assert!(
            s.starts_with("Use rm"),
            "leading heading marker removed: {s}"
        );
    }

    #[test]
    fn clean_text_truncates_on_word_boundary() {
        let s = clean_text("alpha beta gamma delta epsilon", 17);
        assert!(s.chars().count() <= 17, "{s}");
        assert!(s.ends_with('…'));
        assert!(s.starts_with("alpha beta"));
    }

    #[test]
    fn dedupes_ids_and_drops_dangling_edges() {
        let raw = json!({
            "layers": ["A", "B"],
            "nodes": [
                {"id": "a", "label": "A", "layer": 0, "status": "modified", "file": "src/cmds/ls.rs"},
                {"id": "a", "label": "dup", "layer": 0},
                {"id": "a-b", "label": "AB", "layer": 1},
                {"id": "a_b", "label": "AB2", "layer": 1},
                {"id": "lonely", "label": "L", "layer": 1}
            ],
            "edges": [
                {"source": "a", "target": "a-b", "step": 1},
                {"source": "a", "target": "a_b", "step": 2},
                {"source": "a", "target": "ghost", "step": 3},
                {"source": "a", "target": "a", "step": 4},
                {"source": "a", "target": "a-b", "step": 5}
            ]
        });
        let g = normalize(&raw, &paths(), DiagramText::Normal).unwrap();
        let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "a_b", "a_b_2"]);
        assert_eq!(g.edges.len(), 2);
        assert!(g
            .edges
            .iter()
            .all(|e| ids.contains(&e.source.as_str()) && ids.contains(&e.target.as_str())));
        assert_eq!(g.nodes[0].files, vec!["src/cmds/ls.rs".to_string()]);
    }

    #[test]
    fn callee_is_bumped_below_caller_and_rows_are_compacted() {
        // b calls c but both claim layer 1; layer 3 empty; d is on layer 4.
        let raw = json!({
            "layers": ["Entry", "Logic", "Unused", "Gap", "Storage"],
            "nodes": [
                {"id": "a", "layer": 0}, {"id": "b", "layer": 1},
                {"id": "c", "layer": 1}, {"id": "d", "layer": 4}
            ],
            "edges": [
                {"source": "a", "target": "b", "step": 1},
                {"source": "b", "target": "c", "step": 2},
                {"source": "c", "target": "d", "step": 3}
            ]
        });
        let g = normalize(&raw, &[], DiagramText::Short).unwrap();
        let layer = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap().layer;
        assert_eq!(
            (layer("a"), layer("b"), layer("c"), layer("d")),
            (0, 1, 2, 3)
        );
        for e in &g.edges {
            assert!(layer(&e.target) > layer(&e.source));
        }
        assert_eq!(g.layers, vec!["Entry", "Logic", "Logic", "Storage"]);
    }

    #[test]
    fn cycles_are_broken_in_reading_order() {
        let raw = json!({
            "layers": ["X"],
            "nodes": [{"id": "a"}, {"id": "b"}],
            "edges": [
                {"source": "a", "target": "b", "step": 1},
                {"source": "b", "target": "a", "step": 2}
            ]
        });
        let g = normalize(&raw, &[], DiagramText::Short).unwrap();
        assert_eq!(g.edges.len(), 1);
        assert_eq!(
            (g.edges[0].source.as_str(), g.edges[0].target.as_str()),
            ("a", "b")
        );
    }

    #[test]
    fn caps_nodes_and_edges_keeping_changed_components() {
        let mut nodes = vec![json!({"id": "hub", "status": "unchanged", "layer": 1})];
        let mut edges = Vec::new();
        for i in 0..14 {
            nodes.push(json!({"id": format!("m{i}"), "status": "modified", "layer": 0}));
            edges.push(json!({"source": format!("m{i}"), "target": "hub", "step": i + 1}));
        }
        let raw = json!({"layers": ["A", "B"], "nodes": nodes, "edges": edges});
        let g = normalize(&raw, &[], DiagramText::Short).unwrap();
        assert!(g.nodes.len() <= MAX_NODES);
        assert!(g.edges.len() <= MAX_EDGES);
        assert_eq!(
            g.edges.iter().map(|e| e.step).collect::<Vec<_>>(),
            (1..=g.edges.len() as u32).collect::<Vec<_>>()
        );
    }

    #[test]
    fn files_are_restricted_to_the_diff() {
        let raw = json!({
            "layers": ["A", "B"],
            "nodes": [
                {"id": "x", "file": "glob.rs", "files": ["/etc/passwd", "src/core/glob/", "ls.rs"], "layer": 0},
                {"id": "y", "file": "src/db/state.rs", "layer": 1, "status": "unchanged"}
            ],
            "edges": [{"source": "x", "target": "y", "step": 1}]
        });
        let g = normalize(&raw, &paths(), DiagramText::Short).unwrap();
        assert_eq!(
            g.nodes[0].files,
            vec![
                "src/core/glob/tests.rs",
                "src/cmds/ls.rs",
                "src/core/glob.rs"
            ]
        );
        assert!(g.nodes[1].files.is_empty());
        assert_eq!(g.nodes[1].file, "src/db/state.rs");
    }

    #[test]
    fn short_text_drops_details_and_caps_captions() {
        let raw = json!({
            "story": "s",
            "layers": ["A", "B"],
            "nodes": [{"id": "a", "detail": "long detail", "layer": 0}, {"id": "b", "layer": 1}],
            "edges": [{"source": "a", "target": "b", "step": 1,
                "caption": "one two three four five six seven eight nine ten eleven twelve thirteen fourteen"}]
        });
        let g = normalize(&raw, &[], DiagramText::Short).unwrap();
        assert_eq!(g.nodes[0].detail, "");
        assert!(g.edges[0].caption.split_whitespace().count() <= 12);
        let g = normalize(&raw, &[], DiagramText::Normal).unwrap();
        assert_eq!(g.nodes[0].detail, "long detail");
    }

    #[test]
    fn nothing_to_draw_is_none() {
        assert!(normalize(&json!({"nodes": []}), &[], DiagramText::Short).is_none());
        assert!(normalize(
            &json!({"nodes": [{"id": "a"}, {"id": "b"}]}),
            &[],
            DiagramText::Short
        )
        .is_none());
        assert!(normalize(&json!("garbage"), &[], DiagramText::Short).is_none());
    }

    #[test]
    fn real_model_output_normalizes() {
        let raw: Value = serde_json::from_str(include_str!("testdata/graph4544.json")).unwrap();
        let diff_paths = vec![
            "src/cmds/system/ls.rs".to_string(),
            "src/cmds/system/wc_cmd.rs".to_string(),
            "src/cmds/system/search.rs".to_string(),
            "src/cmds/system/find_cmd.rs".to_string(),
            "src/core/operand_glob.rs".to_string(),
        ];
        let g = normalize(&raw, &diff_paths, DiagramText::Normal).unwrap();
        assert_eq!(g.nodes.len(), 7);
        assert_eq!(g.edges.len(), 9);
        // The model left the "Operand parsing" layer empty: compacted away.
        assert_eq!(
            g.layers,
            vec!["Commands", "Glob expansion", "Child process"]
        );
        assert!(!g.story.contains('`'));
        let og = g.nodes.iter().find(|n| n.id == "operand_glob").unwrap();
        assert_eq!(og.files, vec!["src/core/operand_glob.rs"]);
        assert_eq!(og.status, "new");
    }

    #[test]
    fn diff_paths_are_parsed() {
        let d = "diff --git a/x.rs b/x.rs\n+a\ndiff --git a/old name.rs b/new name.rs\n";
        assert_eq!(diff_paths(d), vec!["x.rs", "new name.rs"]);
    }
}
