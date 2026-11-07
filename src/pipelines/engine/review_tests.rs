//! Tests of the review and merge-gate steps (context, collect review,
//! security, verify, risk, approval, status_check), of the built-in
//! templates and of the step limits, on the recording test forge and
//! canned AI responses (no network).

use std::sync::Arc;

use async_trait::async_trait;

use super::state::{RiskTier, Severity};
use super::steps::{Step, StepOutput, StepRegistry};
use super::template::{self, PipelineTemplate, StepDef};
use super::tests_support::{ctx_on, seed_pull};
use super::*;
use crate::git_provider::recording::RecordingForge;
use crate::git_provider::{Capability, PrApproval};

/// One file: line 2 has a bug, line 3 a nit, line 4 a committed token.
const DIFF: &str = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,4 @@\n fn main() {}\n+let x = v[10];\n+let y = 1;\n+let t = \"ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\";\n";

const REVIEW_JSON: &str = r#"{"comments":[
  {"path":"src/lib.rs","line":2,"body":"Index out of bounds when v has fewer than 11 items.","severity":"error","category":"bug"},
  {"path":"src/lib.rs","line":3,"body":"Unused variable.","severity":"info","category":"logic"},
  {"path":"src/lib.rs","line":40,"body":"Not a line of the diff.","severity":"error","category":"bug"}
],"summary":"2 issues"}"#;

const NO_FINDINGS: &str = r#"{"comments":[],"summary":"No issues found."}"#;

fn votes(real: bool) -> String {
    format!(
        r#"{{"votes":[{{"id":1,"real":{real}}},{{"id":2,"real":{real}}},{{"id":3,"real":{real}}},{{"id":99,"real":true}}]}}"#
    )
}

fn features() -> RepoFeatures {
    RepoFeatures {
        collect_prs: true,
        review_prs: true,
        review_post_comments: true,
        ..Default::default()
    }
}

fn forge(caps: Vec<Capability>, approvals: &[(&str, &str)]) -> Arc<RecordingForge> {
    Arc::new(RecordingForge {
        diff: DIFF.into(),
        caps,
        approvals: approvals
            .iter()
            .map(|(login, sha)| PrApproval {
                login: login.to_string(),
                commit_sha: Some(sha.to_string()),
                bot: login.ends_with("[bot]"),
            })
            .collect(),
        ..Default::default()
    })
}

fn all_caps() -> Vec<Capability> {
    vec![Capability::CommitStatus, Capability::PrApprovals]
}

fn cfg() -> Config {
    Config {
        repo_owner: "o".into(),
        repo_name: "r".into(),
        ..Default::default()
    }
}

struct Setup {
    ctx: RunContext,
    gh: Arc<RecordingForge>,
}

fn setup(gh: Arc<RecordingForge>, apply: bool, verify_real: bool) -> Setup {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let config = cfg();
    let db: Arc<dyn DatabaseBackend> = Arc::new(crate::db::Database::open_memory().unwrap());
    let ctx = ctx_on(
        config,
        db,
        Arc::clone(&gh) as Arc<GhClient>,
        ItemKind::Pr,
        Some(7),
        features(),
        apply,
    );
    seed_pull(&ctx, 7, "alice", "main");
    {
        let mut m = ctx.ai_mock.lock().unwrap();
        m.insert("review".into(), REVIEW_JSON.into());
        m.insert("security".into(), NO_FINDINGS.into());
        m.insert("verify".into(), votes(verify_real));
    }
    Setup { ctx, gh }
}

/// The anthropic template without its sync / issue steps (a partial run:
/// the `needs` on `sync-pr` count as met).
fn review_kinds() -> ExecOptions {
    ExecOptions {
        only_kinds: Some(
            [
                "context",
                "review",
                "security",
                "verify",
                "risk",
                "approval",
                "status_check",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ),
        ..Default::default()
    }
}

async fn run_anthropic(ctx: &RunContext) -> RunReport {
    let t = template::builtin(template::ANTHROPIC).unwrap();
    execute_with(t, &StepRegistry::builtin(), ctx, &review_kinds()).await
}

fn step<'a>(r: &'a RunReport, id: &str) -> &'a StepRecord {
    r.steps.iter().find(|s| s.step_id == id).unwrap()
}

#[test]
fn builtin_templates_validate_and_standard_stays_default() {
    let reg = StepRegistry::builtin();
    let names: Vec<&str> = template::builtins()
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names, template::BUILTIN_NAMES);
    for t in template::builtins() {
        template::validate_template(t).unwrap();
        template::validate_template_steps(t, &reg).unwrap();
    }
    assert_eq!(
        template::default_template(&Default::default()),
        template::STANDARD
    );
    // A user template may replace a built-in one, except standard.
    let cfg: template::PipelinesConfig = toml::from_str(
        "[[pipelines]]\nname = \"light\"\n[[pipelines.steps]]\nid = \"a\"\nuses = \"sync\"\n[[pipeline_routes]]\ntemplate = \"anthropic\"\n",
    )
    .unwrap();
    template::validate(&cfg).unwrap();
    assert_eq!(template::find(&cfg, "light").unwrap().steps.len(), 1);
    assert_eq!(template::default_template(&cfg), "anthropic");
}

#[test]
fn anthropic_waves_run_review_and_security_side_by_side() {
    let reg = StepRegistry::builtin();
    let t = template::builtin(template::ANTHROPIC).unwrap();
    assert!(t.parallel);
    let plan = template::plan(t, ItemKind::Pr, &reg);
    let ids: Vec<Vec<&str>> = waves(&plan)
        .iter()
        .map(|w| w.iter().map(|&i| plan[i].id.as_str()).collect())
        .collect();
    assert_eq!(
        ids,
        vec![
            vec!["sync-pr"],
            vec!["context"],
            vec!["review", "security"],
            vec!["verify", "risk"],
            vec!["approval"],
            vec!["status"],
        ]
    );
}

#[tokio::test]
async fn anthropic_flow_posts_only_confirmed_findings_and_blocks() {
    let s = setup(forge(all_caps(), &[("bob", "abc123")]), true, true);
    let r = run_anthropic(&s.ctx).await;
    assert!(r.failure.is_none());
    for id in [
        "context", "review", "security", "verify", "risk", "approval", "status",
    ] {
        assert_eq!(
            step(&r, id).status,
            StepStatus::Done,
            "{id}: {:?}",
            step(&r, id)
        );
    }
    // 2 review findings (the line outside the diff is dropped) + 1 secret.
    assert_eq!(step(&r, "review").findings, 2);
    assert_eq!(step(&r, "security").findings, 1);
    assert_eq!(step(&r, "verify").findings, 3);
    // AI calls: review 1, security 1, verify 3 voters.
    assert_eq!(step(&r, "review").ai_calls, 1);
    assert_eq!(step(&r, "security").ai_calls, 1);
    assert_eq!(step(&r, "verify").ai_calls, 3);
    assert_eq!(step(&r, "risk").ai_calls, 0);
    // An important security finding makes the change high risk: 2 human
    // approvals, bob's counts.
    assert!(step(&r, "risk")
        .summary
        .as_deref()
        .unwrap()
        .starts_with("high"));
    assert_eq!(s.ctx.state().risk.as_ref().unwrap().tier, RiskTier::High);
    let a = s.ctx.state().approval.clone().unwrap();
    assert_eq!((a.required, a.approved), (2, Some(1)));
    // One review with the 3 confirmed findings, and a failing status.
    assert_eq!(s.gh.any_of(&["submit_review"]), ["submit_review 7 3"]);
    let st = s.gh.any_of(&["set_commit_status"]);
    assert_eq!(st.len(), 1);
    assert!(
        st[0].starts_with("set_commit_status abc123 wshm/review failure 2 important"),
        "{st:?}"
    );
    // The stored review is the verified one.
    let stored = s.ctx.db.get_pr_review(7).unwrap().unwrap();
    assert!(stored.posted_to_github);
    assert_eq!(stored.result.comments.len(), 3);

    // The run is recorded with the planned graph.
    let runs = s
        .ctx
        .db
        .list_pipeline_runs(&Default::default(), 10)
        .unwrap();
    assert_eq!(runs[0].template, "anthropic");
    assert_eq!(runs[0].steps.len(), 7);
    assert_eq!(runs[0].ai_calls, 5);

    // A second round does not repeat what was posted.
    let s2 = Setup {
        ctx: ctx_on(
            cfg(),
            Arc::clone(&s.ctx.db),
            Arc::clone(&s.gh) as Arc<GhClient>,
            ItemKind::Pr,
            Some(7),
            features(),
            true,
        ),
        gh: Arc::clone(&s.gh),
    };
    *s2.ctx.ai_mock.lock().unwrap() = s.ctx.ai_mock.lock().unwrap().clone();
    let r2 = run_anthropic(&s2.ctx).await;
    assert!(step(&r2, "verify")
        .summary
        .as_deref()
        .unwrap()
        .contains("nothing new to post"));
    assert_eq!(s.gh.any_of(&["submit_review"]).len(), 1);
}

#[tokio::test]
async fn rejected_findings_are_not_posted_and_missing_approvals_keep_pending() {
    let s = setup(forge(all_caps(), &[]), true, false);
    // Without a security finding the change is medium risk (1 approval).
    s.ctx
        .ai_mock
        .lock()
        .unwrap()
        .insert("review".into(), NO_FINDINGS.into());
    let gh = Arc::new(RecordingForge {
        diff: "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,2 @@\n a\n+b\n".into(),
        caps: all_caps(),
        // The author's own approval and a bot's never count; nor one on an
        // older commit.
        approvals: vec![
            PrApproval { login: "alice".into(), commit_sha: Some("abc123".into()), bot: false },
            PrApproval { login: "ci[bot]".into(), commit_sha: Some("abc123".into()), bot: true },
            PrApproval { login: "bob".into(), commit_sha: Some("old".into()), bot: false },
        ],
        ..Default::default()
    });
    let ctx = ctx_on(
        cfg(),
        Arc::clone(&s.ctx.db),
        Arc::clone(&gh) as Arc<GhClient>,
        ItemKind::Pr,
        Some(7),
        features(),
        true,
    );
    *ctx.ai_mock.lock().unwrap() = s.ctx.ai_mock.lock().unwrap().clone();
    let r = run_anthropic(&ctx).await;
    assert_eq!(step(&r, "verify").findings, 0);
    assert_eq!(ctx.state().approval.clone().unwrap().approved, Some(0));
    assert!(gh.any_of(&["submit_review"]).is_empty());
    let st = gh.any_of(&["set_commit_status"]);
    assert!(
        st[0].contains("pending waiting for 1 more human approval"),
        "{st:?}"
    );
}

#[tokio::test]
async fn dry_run_and_flagged_items_do_not_mutate() {
    // Dry-run.
    let s = setup(forge(all_caps(), &[]), false, true);
    let r = run_anthropic(&s.ctx).await;
    assert!(step(&r, "verify")
        .summary
        .as_deref()
        .unwrap()
        .contains("not posted (dry-run"));
    let status = step(&r, "status");
    assert_eq!(status.status, StepStatus::Skipped);
    assert!(status
        .reason
        .as_deref()
        .unwrap()
        .starts_with("dry-run: would set wshm/review = failure"));
    assert!(s.gh.calls().is_empty(), "{:?}", s.gh.calls());

    // Flagged as a possible prompt injection: nothing posted, no status.
    let s = setup(forge(all_caps(), &[]), true, true);
    s.ctx
        .db
        .upsert_injection_flag(&crate::db::injection::InjectionFlagRow {
            kind: "pull".into(),
            number: 7,
            pipeline: "pr_analysis".into(),
            score: 9,
            reasons: vec!["x".into()],
            findings: Vec::new(),
            blocked: true,
            detected_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();
    let r = run_anthropic(&s.ctx).await;
    assert!(step(&r, "verify")
        .summary
        .as_deref()
        .unwrap()
        .contains("flagged"));
    assert!(step(&r, "status")
        .reason
        .as_deref()
        .unwrap()
        .contains("flagged"));
    assert!(s
        .gh
        .any_of(&["submit_review", "set_commit_status", "label_pr"])
        .is_empty());
}

#[tokio::test]
async fn status_check_needs_the_forge_capability() {
    let s = setup(forge(Vec::new(), &[]), true, true);
    let r = run_anthropic(&s.ctx).await;
    assert!(step(&r, "status")
        .reason
        .as_deref()
        .unwrap()
        .contains("does not support commit statuses"));
    let a = s.ctx.state().approval.clone().unwrap();
    assert_eq!(
        a.approved, None,
        "approvals not counted without the capability"
    );
    assert!(step(&r, "approval")
        .summary
        .as_deref()
        .unwrap()
        .contains("not counted on test"));
}

fn one(src: &str) -> PipelineTemplate {
    let t: PipelineTemplate = toml::from_str(src).unwrap();
    template::validate_template(&t).unwrap();
    template::validate_template_steps(&t, &StepRegistry::builtin()).unwrap();
    t
}

#[tokio::test]
async fn verify_limits_nits_and_voters() {
    let s = setup(forge(all_caps(), &[]), false, true);
    let t = one(r#"
name = "t"
[[steps]]
id = "review"
uses = "review"
with = { mode = "collect" }
[[steps]]
id = "verify"
uses = "verify"
needs = ["review"]
max_ai_calls = 1
with = { voters = 3, max_nits = 0 }
"#);
    let r = execute(&t, &StepRegistry::builtin(), &s.ctx).await;
    let v = step(&r, "verify");
    assert_eq!(v.ai_calls, 1, "the step budget caps the voters");
    assert!(v
        .summary
        .as_deref()
        .unwrap()
        .contains("(1 voter(s), 1 vote(s) needed)"));
    assert_eq!(v.findings, 1, "the nit is dropped (max_nits = 0)");
    let left = s.ctx.state().verified.clone().unwrap();
    assert_eq!(left[0].severity, Severity::Important);
}

#[tokio::test]
async fn max_rounds_caps_a_step_per_item() {
    let s = setup(forge(all_caps(), &[]), false, true);
    let t = one("name = \"t\"\n[[steps]]\nid = \"risk\"\nuses = \"risk\"\nmax_rounds = 2\n");
    let reg = StepRegistry::builtin();
    let fresh = || {
        ctx_on(
            cfg(),
            Arc::clone(&s.ctx.db),
            Arc::clone(&s.gh) as Arc<GhClient>,
            ItemKind::Pr,
            Some(7),
            features(),
            false,
        )
    };
    for _ in 0..2 {
        let r = execute(&t, &reg, &fresh()).await;
        assert_eq!(r.steps[0].status, StepStatus::Done);
    }
    let r = execute(&t, &reg, &fresh()).await;
    assert_eq!(r.steps[0].status, StepStatus::Skipped);
    assert!(r.steps[0]
        .reason
        .as_deref()
        .unwrap()
        .contains("round cap reached"));
    // Another PR has its own rounds.
    let other = ctx_on(
        cfg(),
        Arc::clone(&s.ctx.db),
        Arc::clone(&s.gh) as Arc<GhClient>,
        ItemKind::Pr,
        Some(8),
        features(),
        false,
    );
    assert_eq!(
        execute(&t, &reg, &other).await.steps[0].status,
        StepStatus::Done
    );
}

#[tokio::test]
async fn risk_tiers_and_approval_rules() {
    let docs = "diff --git a/docs/a.md b/docs/a.md\n--- a/docs/a.md\n+++ b/docs/a.md\n@@ -1,1 +1,2 @@\n a\n+b\n";
    let ci = "diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml\n--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n@@ -1,1 +1,2 @@\n a\n+b\n";
    let t = one(r#"
name = "t"
[[steps]]
id = "risk"
uses = "risk"
[[steps]]
id = "approval"
uses = "approval"
needs = ["risk"]
with = { trusted_authors = ["alice"], high = 2, label = "needs-human" }
[[steps]]
id = "high-only"
uses = "risk"
when = { risk = ["high"] }
"#);
    let reg = StepRegistry::builtin();
    for (diff, author, tier, required, high_only) in [
        (docs, "alice", "low", 0, StepStatus::Skipped),
        (docs, "mallory", "low", 1, StepStatus::Skipped),
        (ci, "alice", "high", 2, StepStatus::Done),
    ] {
        let gh = Arc::new(RecordingForge {
            diff: diff.into(),
            caps: all_caps(),
            ..Default::default()
        });
        let s = setup(Arc::clone(&gh), true, true);
        seed_pull(&s.ctx, 7, author, "main");
        let r = execute(&t, &reg, &s.ctx).await;
        assert!(
            step(&r, "risk")
                .summary
                .as_deref()
                .unwrap()
                .starts_with(tier),
            "{diff}"
        );
        assert_eq!(
            s.ctx.state().approval.clone().unwrap().required,
            required,
            "{author} {tier}"
        );
        assert_eq!(
            step(&r, "high-only").status,
            high_only,
            "when.risk reads the risk step"
        );
        let labeled = !gh.any_of(&["label_pr"]).is_empty();
        assert_eq!(
            labeled,
            required > 0,
            "label kept while approvals are missing"
        );
    }
}

#[tokio::test]
async fn context_reads_the_default_branch_checkout() {
    let dir = std::env::temp_dir().join(format!("wshm-ctx-step-{:x}", rand::random::<u64>()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let root = dir.canonicalize().unwrap();
    std::fs::write(
        root.join("CLAUDE.md"),
        "Rust project. Never unwrap in src/.",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn main() {}\nfn other() {}\n").unwrap();
    let s = setup(forge(all_caps(), &[]), false, true);
    let mut config = cfg();
    config.wshm_dir = root.join(".wshm");
    let ctx = ctx_on(
        config,
        Arc::clone(&s.ctx.db),
        Arc::clone(&s.gh) as Arc<GhClient>,
        ItemKind::Pr,
        Some(7),
        features(),
        false,
    );
    let t = one("name = \"t\"\n[[steps]]\nid = \"context\"\nuses = \"context\"\n");
    let r = execute(&t, &StepRegistry::builtin(), &ctx).await;
    assert_eq!(r.steps[0].status, StepStatus::Done);
    assert!(r.steps[0]
        .summary
        .as_deref()
        .unwrap()
        .starts_with("CLAUDE.md;"));
    let c = ctx.state().context.clone().unwrap();
    assert!(c.docs.contains("Never unwrap") && c.docs.contains("<untrusted-data"));
    assert!(c.code.contains("    1 fn main() {}"));
    let p = c.prompt();
    assert!(p.contains("Project guidelines") && p.contains("Code around the changed lines"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn step_params_are_validated() {
    let bad = [
        "uses = \"verify\"\nwith = { voters = 9 }",
        "uses = \"verify\"\nwith = { voters = 1, min_votes = 2 }",
        "uses = \"review\"\nwith = { mode = \"later\" }",
        "uses = \"risk\"\nwith = { high_paths = [\"\"] }",
        "uses = \"approval\"\nwith = { nope = 1 }",
        "uses = \"context\"\nwith = { files = [\"../secret\"] }",
        "uses = \"status_check\"\nwith = { context = \"\" }",
        "uses = \"risk\"\nmax_rounds = 0",
        "uses = \"risk\"\nmax_ai_calls = 0",
    ];
    for b in bad {
        let src = format!("name = \"t\"\n[[steps]]\nid = \"a\"\n{b}\n");
        let t: PipelineTemplate = toml::from_str(&src).unwrap();
        let res = template::validate_template(&t)
            .and_then(|_| template::validate_template_steps(&t, &StepRegistry::builtin()));
        assert!(res.is_err(), "{b}");
    }
}

/// A step that sleeps, to observe parallel waves.
struct Sleep;

#[async_trait]
impl Step for Sleep {
    fn kind(&self) -> &'static str {
        "sleep"
    }
    fn items(&self) -> &'static [ItemKind] {
        &[ItemKind::Issue, ItemKind::Pr]
    }
    async fn run(&self, _ctx: &RunContext, _def: &StepDef) -> anyhow::Result<StepOutput> {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        Ok(StepOutput::done("slept"))
    }
}

#[tokio::test]
async fn parallel_template_runs_independent_steps_together() {
    let mut reg = StepRegistry::default();
    reg.register(Arc::new(Sleep));
    let src = |parallel: bool| {
        format!(
            "name = \"t\"\nparallel = {parallel}\n[[steps]]\nid = \"a\"\nuses = \"sleep\"\n[[steps]]\nid = \"b\"\nuses = \"sleep\"\n[[steps]]\nid = \"c\"\nuses = \"sleep\"\nneeds = [\"a\", \"b\"]\n"
        )
    };
    for (parallel, min_ms, max_ms) in [(true, 600, 850), (false, 900, 5000)] {
        let t: PipelineTemplate = toml::from_str(&src(parallel)).unwrap();
        let s = setup(forge(Vec::new(), &[]), false, true);
        let t0 = std::time::Instant::now();
        let r = execute(&t, &reg, &s.ctx).await;
        let ms = t0.elapsed().as_millis();
        assert!(r.steps.iter().all(|s| s.status == StepStatus::Done));
        assert!(
            (min_ms..max_ms).contains(&ms),
            "parallel={parallel}: {ms} ms"
        );
        // Steps are recorded in plan order.
        let ids: Vec<&str> = r.steps.iter().map(|s| s.step_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }
}
