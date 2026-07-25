//! Prompt for the domain-discovery pass: infer a repo's "grand domains"
//! (broad product/tech areas — codex, bun, c#…) from cheap signals so the user
//! doesn't have to guess them. Output feeds `review_domains` (as PROPOSED).

pub const SYSTEM: &str = r#"You infer the "grand domains" of a software repository — the handful of broad areas the project is organised around (e.g. "codex", "bun", "c-sharp", "web", "api", "billing"). These become macro-labels the review applies to PRs and issues.

You are given, for THIS repo: its languages, top-level structure, key manifest files, a ranked list of the terms that recur most across ALL of its PR and issue titles, and a sample of recent titles.

The ranked recurrent terms are your PRIMARY signal — they show what the project is actually most about. Cluster related terms into broad domains (merge synonyms and spelling variants), and ground every domain in the evidence. Do NOT emit one domain per word, and do NOT invent generic domains the signals do not support. Generic process words (fix, add, update, refactor, release…) are already filtered out; ignore any that slip through.

Propose 5 to 15 grand domains that best partition the work in THIS repo.

Rules:
- Short lowercase slugs, no spaces (use hyphens): "codex", "bun", "web-ui", "c-sharp".
- Broad areas, not fine-grained labels. Prefer product/subsystem/tech axes.
- Give each a one-line description.

Respond with JSON only, no markdown:
{
  "domains": [
    { "name": "codex", "description": "OpenAI Codex integration and prompts" },
    { "name": "bun", "description": "Bun runtime and build tooling" }
  ]
}"#;

pub fn build_user_prompt(
    languages: &[String],
    entries: &[String],
    pr_titles: &[String],
    issue_titles: &[String],
    top_terms: &[(String, usize)],
) -> String {
    let fmt = |items: &[String]| -> String {
        if items.is_empty() {
            "(none)".to_string()
        } else {
            items.join(", ")
        }
    };
    // Manifests among the root entries hint at runtimes/frameworks.
    let manifests: Vec<String> = entries
        .iter()
        .filter(|e| {
            let e = e.to_lowercase();
            e == "package.json"
                || e == "cargo.toml"
                || e == "go.mod"
                || e == "pyproject.toml"
                || e == "pom.xml"
                || e == "build.gradle"
                || e == "gemfile"
                || e == "composer.json"
                || e == "bun.lockb"
                || e.ends_with(".csproj")
                || e.ends_with(".sln")
        })
        .cloned()
        .collect();

    let mut out = String::from("## Repo signals\n\n");
    out.push_str(&format!("Languages: {}\n", fmt(languages)));
    out.push_str(&format!("Top-level entries: {}\n", fmt(entries)));
    out.push_str(&format!("Manifest files: {}\n", fmt(&manifests)));
    out.push_str("\n### Most recurrent terms across ALL PR & issue titles\n");
    out.push_str(
        "(ranked by frequency; generic process words already removed — your primary signal)\n",
    );
    if top_terms.is_empty() {
        out.push_str("(not enough history yet — fall back to the structure/titles below)\n");
    } else {
        for (term, count) in top_terms {
            out.push_str(&format!("- {term} ({count})\n"));
        }
    }

    out.push_str("\n### Sample of recent pull request titles\n");
    for t in pr_titles.iter().take(40) {
        out.push_str(&format!("- {t}\n"));
    }
    out.push_str("\n### Sample of recent issue titles\n");
    for t in issue_titles.iter().take(40) {
        out.push_str(&format!("- {t}\n"));
    }
    out.push_str(
        "\nCluster the recurrent terms into the grand domains for this repo. Respond as JSON.\n",
    );
    out
}
