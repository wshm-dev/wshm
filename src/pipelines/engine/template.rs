//! Pipeline templates, routes and run-retention settings: the `[[pipelines]]`,
//! `[[pipeline_routes]]` and `[pipeline_runs]` config, as read from TOML and
//! from the per-repo DB override (see [`load`]).
//!
//! A template is an ordered list of steps. Each step names a step kind
//! (`uses`), may depend on earlier steps (`needs`: they must be `done`), may
//! run only under conditions (`when`), and has a failure policy and limits.
//! The built-in `standard` template ([`standard`]) reproduces the behaviour
//! wshm had before the engine existed; it is the default and its name is
//! reserved.
//!
//! The pipeline config is only ever read from the TOML (repo `.wshm/config.toml`
//! on the checked-out default branch, or the global config) and from the DB.
//! Never from the content of an issue or PR.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use super::condition::{self, Condition};
use super::steps::StepRegistry;
use super::ItemKind;

/// Name of the built-in default template (reserved).
pub const STANDARD: &str = "standard";
/// Names user templates may not take.
pub const BUILTIN_TEMPLATES: &[&str] = &[STANDARD];

const MAX_STEPS: usize = 64;
const MAX_ROUTES: usize = 128;
const MAX_TEMPLATES: usize = 64;
const MAX_TIMEOUT_SECS: u64 = 24 * 3600;

/// What to do when a step fails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnFailure {
    /// Stop the run: later steps are recorded as skipped and the run (and
    /// the webhook event) is marked failed.
    #[default]
    Abort,
    /// Record the failure and go on with the next steps.
    Continue,
}

fn is_default_on_failure(v: &OnFailure) -> bool {
    *v == OnFailure::Abort
}

/// One step of a template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepDef {
    /// Unique id of the step in its template (`[a-z0-9_-]`, 1-64 chars).
    pub id: String,
    /// Step kind, from the step catalog (`sync`, `triage`, `review`, ...).
    pub uses: String,
    /// Ids of earlier steps that must have finished `done` for this one to
    /// run. Otherwise the step is skipped with the reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// Conditions; the step is skipped (with the reason) when one fails.
    #[serde(default, skip_serializing_if = "Condition::is_empty")]
    pub when: Condition,
    #[serde(default, skip_serializing_if = "is_default_on_failure")]
    pub on_failure: OnFailure,
    /// Abort the step (status failed) after this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Step-specific parameters, validated by the step kind. Never put a
    /// secret here: steps read secrets from the secret store by name.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub with: serde_json::Map<String, serde_json::Value>,
}

/// `[[pipelines]]`: a named, ordered list of steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineTemplate {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// AI call budget of a run: once reached, the remaining steps are
    /// skipped. Unset = no budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ai_calls: Option<u32>,
    pub steps: Vec<StepDef>,
}

/// `[[pipeline_routes]]`: picks the template of a run. Routes are tried by
/// descending `priority` (then in declaration order); the first whose
/// conditions all hold wins. No match = `standard`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineRoute {
    pub template: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default, skip_serializing_if = "Condition::is_empty")]
    pub when: Condition,
}

/// `[pipeline_runs]`: bounded retention of the run history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineRunsConfig {
    /// Delete runs older than this many days (0 = no age limit).
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    /// Keep at most this many runs per issue/PR, newest first (0 = no limit).
    #[serde(default = "default_max_per_item")]
    pub max_per_item: u32,
}

fn default_retention_days() -> u32 {
    30
}

fn default_max_per_item() -> u32 {
    20
}

impl Default for PipelineRunsConfig {
    fn default() -> Self {
        Self {
            retention_days: default_retention_days(),
            max_per_item: default_max_per_item(),
        }
    }
}

/// The repo's effective pipeline config (TOML, with the DB override applied
/// by [`load`]). Field names are the TOML keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelinesConfig {
    #[serde(default)]
    pub pipelines: Vec<PipelineTemplate>,
    #[serde(default)]
    pub pipeline_routes: Vec<PipelineRoute>,
    #[serde(default)]
    pub pipeline_runs: PipelineRunsConfig,
}

/// Source of the built-in `standard` template. Kept as TOML so it reads like
/// a user template and goes through the same parser and validation.
///
/// It mirrors the event handling wshm had before the engine:
/// - issue: sync (`collect_issues`), then triage (`triage_issues` + the
///   repo filters), then the ICM memory;
/// - PR: sync (`collect_prs`), then the inline review (`review_prs`, its
///   failure does not stop the run), then the analysis (`analyze_prs` + the
///   repo filters; diagrams are spawned in the background), then the memory.
pub const STANDARD_TOML: &str = r#"
name = "standard"
description = "Default: the behaviour of the repo toggles (triage_issues, review_prs, analyze_prs, ...)."

[[steps]]
id = "sync-issue"
uses = "sync"
when = { item = "issue", features = ["collect_issues"] }

[[steps]]
id = "triage"
uses = "triage"
needs = ["sync-issue"]
when = { item = "issue", features = ["triage_issues"], repo_filters = true }

[[steps]]
id = "triage-memory"
uses = "memory"
needs = ["triage"]
when = { item = "issue" }

[[steps]]
id = "sync-pr"
uses = "sync"
when = { item = "pr", features = ["collect_prs"] }

[[steps]]
id = "review"
uses = "review"
needs = ["sync-pr"]
on_failure = "continue"
when = { item = "pr", features = ["review_prs"] }

[[steps]]
id = "analyze"
uses = "analyze"
needs = ["sync-pr"]
when = { item = "pr", features = ["analyze_prs"], repo_filters = true }

[[steps]]
id = "analysis-memory"
uses = "memory"
needs = ["analyze"]
when = { item = "pr" }
"#;

static STANDARD_TEMPLATE: LazyLock<PipelineTemplate> = LazyLock::new(|| {
    // Safe: STANDARD_TOML is a constant covered by tests.
    toml::from_str(STANDARD_TOML).expect("built-in standard template must parse")
});

/// The built-in `standard` template.
pub fn standard() -> &'static PipelineTemplate {
    &STANDARD_TEMPLATE
}

/// A built-in template by name.
pub fn builtin(name: &str) -> Option<&'static PipelineTemplate> {
    (name == STANDARD).then(standard)
}

fn valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Structural validation of one template: names, ids, `needs` order, item
/// scopes, conditions and limits. Step kinds are checked by
/// [`validate_with_registry`] (the catalog depends on the edition).
pub fn validate_template(t: &PipelineTemplate) -> Result<(), String> {
    let ctx = |m: String| format!("pipeline '{}': {m}", t.name);
    if !valid_ident(&t.name) {
        return Err(format!(
            "invalid pipeline name '{}' (use 1-64 chars of a-z, 0-9, '-', '_')",
            t.name
        ));
    }
    if t.steps.is_empty() {
        return Err(ctx("has no steps".into()));
    }
    if t.steps.len() > MAX_STEPS {
        return Err(ctx(format!("more than {MAX_STEPS} steps")));
    }
    if t.max_ai_calls == Some(0) {
        return Err(ctx(
            "max_ai_calls must be at least 1 (omit it for no budget)".into(),
        ));
    }
    let mut seen: HashMap<&str, &StepDef> = HashMap::new();
    for s in &t.steps {
        let sctx = |m: String| ctx(format!("step '{}': {m}", s.id));
        if !valid_ident(&s.id) {
            return Err(ctx(format!(
                "invalid step id '{}' (use 1-64 chars of a-z, 0-9, '-', '_')",
                s.id
            )));
        }
        if seen.contains_key(s.id.as_str()) {
            return Err(ctx(format!("duplicate step id '{}'", s.id)));
        }
        if !valid_ident(&s.uses) {
            return Err(sctx(format!("invalid step kind '{}'", s.uses)));
        }
        let mut needs_seen = HashSet::new();
        for n in &s.needs {
            if !needs_seen.insert(n) {
                return Err(sctx(format!("'{n}' listed twice in needs")));
            }
            let Some(dep) = seen.get(n.as_str()) else {
                return Err(sctx(format!(
                    "needs '{n}', which is not an earlier step of the pipeline"
                )));
            };
            if let (Some(a), Some(b)) = (s.when.item, dep.when.item) {
                if a != b {
                    return Err(sctx(format!(
                        "runs on {} but needs '{n}', which only runs on {}",
                        a.as_str(),
                        b.as_str()
                    )));
                }
            }
        }
        if let Some(secs) = s.timeout_secs {
            if secs == 0 || secs > MAX_TIMEOUT_SECS {
                return Err(sctx(format!(
                    "timeout_secs must be between 1 and {MAX_TIMEOUT_SECS}"
                )));
            }
        }
        condition::validate(&s.when).map_err(sctx)?;
        seen.insert(&s.id, s);
    }
    Ok(())
}

/// Structural validation of a whole pipeline config.
pub fn validate(cfg: &PipelinesConfig) -> Result<(), String> {
    if cfg.pipelines.len() > MAX_TEMPLATES {
        return Err(format!("more than {MAX_TEMPLATES} pipelines"));
    }
    if cfg.pipeline_routes.len() > MAX_ROUTES {
        return Err(format!("more than {MAX_ROUTES} pipeline routes"));
    }
    let mut names = HashSet::new();
    for t in &cfg.pipelines {
        if BUILTIN_TEMPLATES.contains(&t.name.as_str()) {
            return Err(format!(
                "pipeline name '{}' is reserved for the built-in template",
                t.name
            ));
        }
        if !names.insert(t.name.as_str()) {
            return Err(format!("duplicate pipeline name '{}'", t.name));
        }
        validate_template(t)?;
    }
    for (i, r) in cfg.pipeline_routes.iter().enumerate() {
        if !names.contains(r.template.as_str()) && builtin(&r.template).is_none() {
            return Err(format!(
                "pipeline route #{}: unknown pipeline '{}'",
                i + 1,
                r.template
            ));
        }
        condition::validate(&r.when).map_err(|e| format!("pipeline route #{}: {e}", i + 1))?;
    }
    Ok(())
}

/// Check every template against the step catalog: the kind exists, can run
/// on the item scope the step declares, and accepts its `with` parameters.
pub fn validate_with_registry(
    templates: &[PipelineTemplate],
    registry: &StepRegistry,
) -> Result<(), String> {
    for t in templates {
        validate_template_steps(t, registry)?;
    }
    Ok(())
}

/// Step-kind checks of one template (see [`validate_with_registry`]).
pub fn validate_template_steps(
    t: &PipelineTemplate,
    registry: &StepRegistry,
) -> Result<(), String> {
    for s in &t.steps {
        let err = |m: String| format!("pipeline '{}': step '{}': {m}", t.name, s.id);
        let Some(kind) = registry.get(&s.uses) else {
            return Err(err(format!(
                "unknown step kind '{}' (known: {})",
                s.uses,
                registry.kinds().join(", ")
            )));
        };
        if let Some(item) = s.when.item {
            if !kind.items().contains(&item) {
                return Err(err(format!(
                    "step kind '{}' does not run on {}",
                    s.uses,
                    item.as_str()
                )));
            }
        }
        kind.validate_params(&s.with).map_err(err)?;
    }
    Ok(())
}

/// Pick the template for an item: routes by descending priority (stable on
/// declaration order), first match wins; `standard` when none matches.
/// Route conditions are evaluated by `matches`.
pub async fn select<'a, F, Fut>(cfg: &'a PipelinesConfig, mut matches: F) -> &'a str
where
    F: FnMut(&'a Condition) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let mut routes: Vec<&PipelineRoute> = cfg.pipeline_routes.iter().collect();
    routes.sort_by_key(|r| std::cmp::Reverse(r.priority));
    for r in routes {
        if matches(&r.when).await {
            return &r.template;
        }
    }
    STANDARD
}

/// Find a template by name among the user templates and the built-ins.
pub fn find<'a>(cfg: &'a PipelinesConfig, name: &str) -> Option<&'a PipelineTemplate> {
    cfg.pipelines
        .iter()
        .find(|t| t.name == name)
        .or_else(|| builtin(name))
}

/// Steps of `t` that apply to `item` (by `when.item` and by what the step
/// kind supports), in template order.
pub fn plan<'a>(
    t: &'a PipelineTemplate,
    item: ItemKind,
    registry: &StepRegistry,
) -> Vec<&'a StepDef> {
    t.steps
        .iter()
        .filter(|s| s.when.item.is_none_or(|i| i == item))
        .filter(|s| {
            registry
                .get(&s.uses)
                .is_none_or(|k| k.items().contains(&item))
        })
        .collect()
}

// ── Config sources: TOML + DB override ──────────────────────────────

/// The TOML part of the config, as a [`PipelinesConfig`].
pub fn from_config(config: &crate::config::Config) -> PipelinesConfig {
    PipelinesConfig {
        pipelines: config.pipelines.clone(),
        pipeline_routes: config.pipeline_routes.clone(),
        pipeline_runs: config.pipeline_runs.clone(),
    }
}

/// Apply a DB override (JSON object with any of `pipelines`,
/// `pipeline_routes`, `pipeline_runs`) over the TOML value: the two lists
/// are replaced as a whole, `pipeline_runs` is merged key by key.
pub fn apply_override(
    toml: &PipelinesConfig,
    over: &serde_json::Value,
) -> Result<PipelinesConfig, String> {
    if !over.is_object() {
        return Err("expected a JSON object or null".into());
    }
    let reference = serde_json::to_value(PipelinesConfig::default()).unwrap_or_default();
    let unknown = crate::config::unknown_json_keys(&reference, over);
    if !unknown.is_empty() {
        return Err(format!("unknown setting(s): {}", unknown.join(", ")));
    }
    let mut base = serde_json::to_value(toml).map_err(|e| e.to_string())?;
    crate::config::merge_json(&mut base, over);
    let merged: PipelinesConfig =
        serde_json::from_value(base).map_err(|e| format!("invalid pipeline settings: {e}"))?;
    validate(&merged)?;
    Ok(merged)
}

/// Validate an override as the API receives it: valid on its own and once
/// merged, with every step kind known to `registry`.
pub fn validate_override(
    toml: &PipelinesConfig,
    over: &serde_json::Value,
    registry: &StepRegistry,
) -> Result<PipelinesConfig, String> {
    let merged = apply_override(toml, over)?;
    validate_with_registry(&merged.pipelines, registry)?;
    Ok(merged)
}

/// The raw DB override, if one is set.
pub fn read_override(db: &dyn crate::db::backend::DatabaseBackend) -> Option<serde_json::Value> {
    db.get_app_setting(crate::db::settings::PIPELINES_KEY)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty() && s.trim() != "null")
        .and_then(|s| match serde_json::from_str::<serde_json::Value>(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("Ignoring unparsable pipelines override in the DB: {e}");
                None
            }
        })
}

/// The repo's effective pipeline config: the DB override (shared by every
/// pod, read on each run) applied over the TOML. An invalid override is
/// logged and ignored.
pub fn load(
    config: &crate::config::Config,
    db: &dyn crate::db::backend::DatabaseBackend,
) -> PipelinesConfig {
    let toml = from_config(config);
    match read_override(db) {
        None => toml,
        Some(over) => match apply_override(&toml, &over) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Ignoring invalid pipelines override in the DB: {e}");
                toml
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> PipelinesConfig {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn standard_template_parses_and_validates() {
        let t = standard();
        assert_eq!(t.name, STANDARD);
        validate_template(t).unwrap();
        validate_template_steps(t, &StepRegistry::builtin()).unwrap();
        let ids: Vec<&str> = t.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "sync-issue",
                "triage",
                "triage-memory",
                "sync-pr",
                "review",
                "analyze",
                "analysis-memory"
            ]
        );
    }

    #[test]
    fn standard_plan_per_item() {
        let reg = StepRegistry::builtin();
        let issue: Vec<&str> = plan(standard(), ItemKind::Issue, &reg)
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(issue, ["sync-issue", "triage", "triage-memory"]);
        let pr: Vec<&str> = plan(standard(), ItemKind::Pr, &reg)
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(pr, ["sync-pr", "review", "analyze", "analysis-memory"]);
    }

    #[test]
    fn parses_full_toml_example() {
        let cfg = parse(
            r#"
[[pipelines]]
name = "light"
description = "docs and small PRs"
max_ai_calls = 3

[[pipelines.steps]]
id = "sync"
uses = "sync"

[[pipelines.steps]]
id = "analyze"
uses = "analyze"
needs = ["sync"]
on_failure = "continue"
timeout_secs = 300
when = { item = "pr", labels_none = ["skip-ai"], paths = ["docs/**", "*.md"] }

[[pipeline_routes]]
template = "light"
priority = 10
when = { item = "pr", labels_any = ["docs"] }

[pipeline_runs]
retention_days = 7
"#,
        );
        validate(&cfg).unwrap();
        validate_with_registry(&cfg.pipelines, &StepRegistry::builtin()).unwrap();
        let t = &cfg.pipelines[0];
        assert_eq!(t.max_ai_calls, Some(3));
        assert_eq!(t.steps[1].on_failure, OnFailure::Continue);
        assert_eq!(t.steps[1].timeout_secs, Some(300));
        assert_eq!(t.steps[1].when.paths, ["docs/**", "*.md"]);
        assert_eq!(cfg.pipeline_runs.retention_days, 7);
        assert_eq!(
            cfg.pipeline_runs.max_per_item, 20,
            "unset key keeps default"
        );
        assert_eq!(cfg.pipeline_routes[0].priority, 10);
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<PipelinesConfig>(
            "[[pipelines]]\nname = \"x\"\n[[pipelines.steps]]\nid = \"a\"\nuses = \"sync\"\nbogus = 1\n"
        )
        .is_err());
        assert!(toml::from_str::<PipelinesConfig>(
            "[[pipelines]]\nname = \"x\"\n[[pipelines.steps]]\nid = \"a\"\nuses = \"sync\"\nwhen = { lables_any = [\"x\"] }\n"
        )
        .is_err());
    }

    fn one(steps: &str) -> Result<(), String> {
        let cfg: PipelinesConfig = toml::from_str(&format!("[[pipelines]]\nname = \"t\"\n{steps}"))
            .map_err(|e| e.to_string())?;
        validate(&cfg)?;
        validate_with_registry(&cfg.pipelines, &StepRegistry::builtin())
    }

    #[test]
    fn validation_errors() {
        let s = |id: &str, extra: &str| {
            format!("[[pipelines.steps]]\nid = \"{id}\"\nuses = \"sync\"\n{extra}\n")
        };
        assert!(one(&s("a", "")).is_ok());
        // Duplicate id.
        assert!(one(&(s("a", "") + &s("a", "")))
            .unwrap_err()
            .contains("duplicate"));
        // needs: unknown, later step, self.
        assert!(one(&s("a", "needs = [\"zz\"]")).is_err());
        assert!(one(&(s("a", "needs = [\"b\"]") + &s("b", ""))).is_err());
        assert!(one(&s("a", "needs = [\"a\"]")).is_err());
        // Item scope mismatch between a step and what it needs.
        assert!(one(&(s("a", "when = { item = \"issue\" }")
            + &s("b", "needs = [\"a\"]\nwhen = { item = \"pr\" }")))
        .unwrap_err()
        .contains("only runs on issue"));
        // Bad ids / kinds.
        assert!(one(&s("A b", "")).is_err());
        assert!(one("[[pipelines.steps]]\nid = \"a\"\nuses = \"nope\"\n")
            .unwrap_err()
            .contains("unknown step kind"));
        // A step kind on an item it does not support.
        assert!(one(
            "[[pipelines.steps]]\nid = \"a\"\nuses = \"triage\"\nwhen = { item = \"pr\" }\n"
        )
        .unwrap_err()
        .contains("does not run on pr"));
        // Params the kind does not accept.
        assert!(one(&s("a", "with = { x = 1 }")).is_err());
        // Limits.
        assert!(one(&s("a", "timeout_secs = 0")).is_err());
        assert!(one(&s("a", "when = { features = [\"nope\"] }"))
            .unwrap_err()
            .contains("unknown feature"));
        assert!(one(&s("a", "when = { paths = [\"\"] }")).is_err());
        // No steps.
        assert!(one("steps = []").unwrap_err().contains("no steps"));
    }

    #[test]
    fn reserved_and_duplicate_names() {
        let step = "[[pipelines.steps]]\nid = \"a\"\nuses = \"sync\"\n";
        let cfg = parse(&format!("[[pipelines]]\nname = \"standard\"\n{step}"));
        assert!(validate(&cfg).unwrap_err().contains("reserved"));
        let cfg = parse(&format!(
            "[[pipelines]]\nname = \"x\"\n{step}[[pipelines]]\nname = \"x\"\n{step}"
        ));
        assert!(validate(&cfg).unwrap_err().contains("duplicate"));
        let cfg = parse("[[pipeline_routes]]\ntemplate = \"ghost\"\n");
        assert!(validate(&cfg).unwrap_err().contains("unknown pipeline"));
        let cfg = parse("[[pipeline_routes]]\ntemplate = \"standard\"\n");
        validate(&cfg).unwrap();
    }

    #[tokio::test]
    async fn route_selection_by_priority_then_order() {
        let step = "[[pipelines.steps]]\nid = \"a\"\nuses = \"sync\"\n";
        let cfg = parse(&format!(
            "[[pipelines]]\nname = \"a\"\n{step}[[pipelines]]\nname = \"b\"\n{step}[[pipelines]]\nname = \"c\"\n{step}
[[pipeline_routes]]
template = \"a\"
when = {{ labels_any = [\"x\"] }}
[[pipeline_routes]]
template = \"b\"
priority = 5
when = {{ labels_any = [\"x\"] }}
[[pipeline_routes]]
template = \"c\"
priority = 5
when = {{ labels_any = [\"x\"] }}
"
        ));
        validate(&cfg).unwrap();
        // b and c share the top priority: declaration order breaks the tie.
        assert_eq!(select(&cfg, |_| async { true }).await, "b");
        assert_eq!(select(&cfg, |_| async { false }).await, STANDARD);
        assert_eq!(
            select(&PipelinesConfig::default(), |_| async { true }).await,
            STANDARD
        );
    }

    #[test]
    fn db_override_replaces_lists_and_merges_runs() {
        let toml = parse(
            "[[pipelines]]\nname = \"x\"\n[[pipelines.steps]]\nid = \"a\"\nuses = \"sync\"\n[pipeline_runs]\nretention_days = 9\n",
        );
        let over = serde_json::json!({
            "pipelines": [],
            "pipeline_runs": {"max_per_item": 3}
        });
        let eff = apply_override(&toml, &over).unwrap();
        assert!(eff.pipelines.is_empty());
        assert_eq!(eff.pipeline_runs.retention_days, 9);
        assert_eq!(eff.pipeline_runs.max_per_item, 3);
        assert!(apply_override(&toml, &serde_json::json!({"nope": 1})).is_err());
        assert!(apply_override(&toml, &serde_json::json!([1])).is_err());
        // A route to a template the override removed is invalid once merged.
        let toml = parse(
            "[[pipelines]]\nname = \"x\"\n[[pipelines.steps]]\nid = \"a\"\nuses = \"sync\"\n[[pipeline_routes]]\ntemplate = \"x\"\n",
        );
        assert!(apply_override(&toml, &serde_json::json!({"pipelines": []})).is_err());
    }

    #[test]
    fn load_prefers_valid_db_override() {
        let db = crate::db::Database::open_memory().unwrap();
        let config = crate::config::Config::default();
        assert_eq!(load(&config, &db), PipelinesConfig::default());
        db.set_app_setting(
            crate::db::settings::PIPELINES_KEY,
            r#"{"pipeline_runs":{"retention_days":2}}"#,
        )
        .unwrap();
        assert_eq!(load(&config, &db).pipeline_runs.retention_days, 2);
        db.set_app_setting(crate::db::settings::PIPELINES_KEY, r#"{"bad":1}"#)
            .unwrap();
        assert_eq!(load(&config, &db), PipelinesConfig::default());
    }
}
