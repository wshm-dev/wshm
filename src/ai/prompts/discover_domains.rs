//! Prompt for the domain-discovery pass: infer a repo's "grand domains"
//! (broad product/tech areas — codex, bun, c#…) from cheap signals so the user
//! doesn't have to guess them. Output feeds `review_domains` (as PROPOSED).

pub const SYSTEM: &str = r#"You infer the "grand domains" of a software repository — the handful of broad areas the project is organised around (e.g. "codex", "bun", "c#", "web", "api", "billing"). These become macro-labels the review applies to PRs and issues.

You are given the repo's languages, top-level structure, key manifest files, and a sample of recent PR/issue titles. Propose 5 to 15 grand domains that best partition the work in THIS repo.

Rules:
- Short lowercase slugs (e.g. "codex", "bun", "c#", "web-ui"), no spaces (use hyphens).
- Broad areas, not fine-grained labels. Prefer product/subsystem/tech axes.
- Ground them in the signals; do NOT invent generic domains that don't fit.
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
    out.push_str("\n### Recent pull request titles\n");
    for t in pr_titles.iter().take(40) {
        out.push_str(&format!("- {t}\n"));
    }
    out.push_str("\n### Recent issue titles\n");
    for t in issue_titles.iter().take(40) {
        out.push_str(&format!("- {t}\n"));
    }
    out.push_str("\nPropose the grand domains for this repo as JSON.\n");
    out
}
