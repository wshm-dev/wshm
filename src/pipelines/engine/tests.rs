//! Engine tests: execution semantics, run recording, and equivalence of the
//! `standard` template with the event flow wshm had before the engine.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::steps::{Step, StepOutput, StepRegistry};
use super::template::{self, PipelineTemplate, StepDef};
use super::tests_support::{ctx_with, seed_issue, seed_pull};
use super::*;
use crate::db::runs::PipelineRunFilter;

/// Shared state of the fake steps of one run.
#[derive(Default)]
struct World {
    log: Mutex<Vec<&'static str>>,
    /// The forge returns the item (else `sync` skips).
    found: bool,
    /// The item is caught by the repo filters (author in skip_authors).
    filtered: bool,
    /// Step kinds that fail.
    fail: HashSet<&'static str>,
    /// AI calls made by each triage / analyze / review.
    ai_calls: u32,
    /// Sleep of every step, in ms.
    sleep_ms: u64,
    /// Background AI-call sink captured by `analyze`.
    sink: Mutex<Option<AiCallSink>>,
    /// Budget seen by `analyze`.
    budget_seen: Mutex<Option<Option<u32>>>,
}

struct Fake {
    kind: &'static str,
    items: &'static [ItemKind],
    world: Arc<World>,
}

#[async_trait]
impl Step for Fake {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn items(&self) -> &'static [ItemKind] {
        self.items
    }

    async fn run(&self, ctx: &RunContext, _def: &StepDef) -> anyhow::Result<StepOutput> {
        self.world.log.lock().unwrap().push(self.kind);
        if self.world.sleep_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.world.sleep_ms)).await;
        }
        if matches!(self.kind, "triage" | "analyze" | "review") {
            for _ in 0..self.world.ai_calls {
                note_ai_call();
            }
        }
        if self.world.fail.contains(self.kind) {
            anyhow::bail!("{} exploded", self.kind);
        }
        if self.kind == "analyze" {
            *self.world.sink.lock().unwrap() = ctx.ai_call_sink();
            *self.world.budget_seen.lock().unwrap() = Some(ctx.budget_left());
        }
        if self.kind == "sync" {
            if !self.world.found {
                return Ok(StepOutput::skipped("not returned by the forge"));
            }
            let author = if self.world.filtered { "bot" } else { "human" };
            let n = ctx.item.number.unwrap();
            match ctx.item.kind {
                ItemKind::Issue => seed_issue(ctx, n, &[], author),
                ItemKind::Pr => seed_pull(ctx, n, author, "main"),
            }
        }
        let out = StepOutput::done(format!("{} ok", self.kind));
        Ok(if self.kind == "review" {
            out.with_findings(3)
        } else {
            out
        })
    }
}

fn fake_registry(world: &Arc<World>) -> StepRegistry {
    const BOTH: &[ItemKind] = &[ItemKind::Issue, ItemKind::Pr];
    const ISSUE: &[ItemKind] = &[ItemKind::Issue];
    const PR: &[ItemKind] = &[ItemKind::Pr];
    let mut r = StepRegistry::default();
    for (kind, items) in [
        ("sync", BOTH),
        ("triage", ISSUE),
        ("review", PR),
        ("analyze", PR),
        ("memory", BOTH),
    ] {
        r.register(Arc::new(Fake {
            kind,
            items,
            world: Arc::clone(world),
        }));
    }
    r
}

fn features(
    collect: bool,
    triage: bool,
    review: bool,
    analyze: bool,
    filters_on: bool,
) -> RepoFeatures {
    let mut f = RepoFeatures {
        collect_issues: collect,
        collect_prs: collect,
        triage_issues: triage,
        review_prs: review,
        analyze_prs: analyze,
        ..Default::default()
    };
    if filters_on {
        f.filters.skip_authors = vec!["bot".into()];
    }
    f
}

/// The issue flow of the event processor before the engine (post admission
/// checks): which steps ran, and whether the event failed.
fn legacy_issue(f: &RepoFeatures, w: &World) -> (Vec<&'static str>, bool) {
    let mut ran = Vec::new();
    if !f.collect_issues {
        return (ran, false);
    }
    ran.push("sync");
    if w.fail.contains("sync") {
        return (ran, true);
    }
    if !w.found || !f.triage_issues || w.filtered {
        return (ran, false);
    }
    ran.push("triage");
    if w.fail.contains("triage") {
        return (ran, true);
    }
    ran.push("memory");
    (ran, w.fail.contains("memory"))
}

/// Same for the PR flow.
fn legacy_pr(f: &RepoFeatures, w: &World) -> (Vec<&'static str>, bool) {
    let mut ran = Vec::new();
    if !f.collect_prs {
        return (ran, false);
    }
    ran.push("sync");
    if w.fail.contains("sync") {
        return (ran, true);
    }
    if !w.found {
        return (ran, false);
    }
    if f.review_prs {
        ran.push("review"); // a review failure is only logged
    }
    if !f.analyze_prs || w.filtered {
        return (ran, false);
    }
    ran.push("analyze");
    if w.fail.contains("analyze") {
        return (ran, true);
    }
    ran.push("memory");
    (ran, w.fail.contains("memory"))
}

fn bools() -> [bool; 2] {
    [false, true]
}

#[tokio::test]
async fn standard_template_matches_legacy_issue_flow() {
    let mut cases = 0;
    for collect in bools() {
        for triage in bools() {
            for found in bools() {
                for filtered in bools() {
                    for fail in [None, Some("sync"), Some("triage"), Some("memory")] {
                        let world = Arc::new(World {
                            found,
                            filtered,
                            fail: fail.into_iter().collect(),
                            ..Default::default()
                        });
                        let f = features(collect, triage, false, false, true);
                        let (want, want_err) = legacy_issue(&f, &world);
                        let ctx = ctx_with(ItemKind::Issue, Some(9), f);
                        let report =
                            execute(template::standard(), &fake_registry(&world), &ctx).await;
                        let got = world.log.lock().unwrap().clone();
                        let case = format!(
                            "collect={collect} triage={triage} found={found} filtered={filtered} fail={fail:?}"
                        );
                        assert_eq!(got, want, "{case}");
                        assert_eq!(report.into_result().is_err(), want_err, "{case}");
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 64);
}

#[tokio::test]
async fn standard_template_matches_legacy_pr_flow() {
    let mut cases = 0;
    for collect in bools() {
        for review in bools() {
            for analyze in bools() {
                for found in bools() {
                    for filtered in bools() {
                        for fail in [
                            None,
                            Some("sync"),
                            Some("review"),
                            Some("analyze"),
                            Some("memory"),
                        ] {
                            let world = Arc::new(World {
                                found,
                                filtered,
                                fail: fail.into_iter().collect(),
                                ..Default::default()
                            });
                            let f = features(collect, false, review, analyze, true);
                            let (want, want_err) = legacy_pr(&f, &world);
                            let ctx = ctx_with(ItemKind::Pr, Some(4), f);
                            let report =
                                execute(template::standard(), &fake_registry(&world), &ctx).await;
                            let got = world.log.lock().unwrap().clone();
                            let case = format!(
                                "collect={collect} review={review} analyze={analyze} found={found} filtered={filtered} fail={fail:?}"
                            );
                            assert_eq!(got, want, "{case}");
                            assert_eq!(report.into_result().is_err(), want_err, "{case}");
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 160);
}

#[tokio::test]
async fn standard_reasons_name_the_toggle() {
    let world = Arc::new(World {
        found: true,
        ..Default::default()
    });
    let ctx = ctx_with(
        ItemKind::Pr,
        Some(1),
        features(true, false, false, true, false),
    );
    let report = execute(template::standard(), &fake_registry(&world), &ctx).await;
    let review = &report.steps[1];
    assert_eq!(review.step_id, "review");
    assert_eq!(review.status, StepStatus::Skipped);
    assert_eq!(review.reason.as_deref(), Some("review_prs disabled"));
    assert_eq!(report.status("analyze"), Some(StepStatus::Done));

    let world = Arc::new(World::default());
    let ctx = ctx_with(
        ItemKind::Issue,
        Some(1),
        features(false, true, false, false, false),
    );
    let report = execute(template::standard(), &fake_registry(&world), &ctx).await;
    assert_eq!(
        report.steps[0].reason.as_deref(),
        Some("collect_issues disabled")
    );
    assert_eq!(
        report.steps[1].reason.as_deref(),
        Some("needs 'sync-issue', which was skipped")
    );
}

#[tokio::test]
async fn run_is_recorded_per_step() {
    let world = Arc::new(World {
        found: true,
        ai_calls: 2,
        fail: ["review"].into_iter().collect(),
        ..Default::default()
    });
    let mut ctx = ctx_with(
        ItemKind::Pr,
        Some(12),
        features(true, false, true, true, false),
    );
    ctx.trigger = "pull_request.opened".into();
    ctx.event_id = Some(77);
    let report = execute(template::standard(), &fake_registry(&world), &ctx).await;
    assert!(report.failure.is_none(), "review failure does not abort");

    let runs = ctx
        .db
        .list_pipeline_runs(
            &PipelineRunFilter {
                item_kind: Some("pr".into()),
                item_number: Some(12),
            },
            10,
        )
        .unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.id, report.run_id.unwrap());
    assert_eq!(run.template, "standard");
    assert_eq!(run.trigger, "pull_request.opened");
    assert_eq!(run.event_id, Some(77));
    assert_eq!(run.status, "done");
    assert_eq!(run.head_sha.as_deref(), Some("abc123"), "head after sync");
    assert_eq!(run.ai_calls, 4, "review 2 (then failed) + analyze 2");
    let plan: Vec<&str> = run.plan.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(plan, ["sync-pr", "review", "analyze", "analysis-memory"]);
    let steps: Vec<(&str, &str)> = run
        .steps
        .iter()
        .map(|s| (s.step_id.as_str(), s.status.as_str()))
        .collect();
    assert_eq!(
        steps,
        [
            ("sync-pr", "done"),
            ("review", "failed"),
            ("analyze", "done"),
            ("analysis-memory", "done")
        ]
    );
    assert_eq!(run.steps[1].reason.as_deref(), Some("review exploded"));
    assert_eq!(run.steps[1].ai_calls, 2);
    assert_eq!(run.steps[2].summary.as_deref(), Some("analyze ok"));
    assert_eq!(run.steps[2].ai_calls, 2);
    assert!(run.finished_at.is_some() && run.duration_ms.is_some());
}

#[tokio::test]
async fn abort_skips_the_rest_and_fails_the_run() {
    let world = Arc::new(World {
        found: true,
        fail: ["triage"].into_iter().collect(),
        ..Default::default()
    });
    let ctx = ctx_with(
        ItemKind::Issue,
        Some(5),
        features(true, true, false, false, false),
    );
    let report = execute(template::standard(), &fake_registry(&world), &ctx).await;
    assert_eq!(report.steps[2].status, StepStatus::Skipped);
    assert_eq!(
        report.steps[2].reason.as_deref(),
        Some("run aborted: step 'triage' failed")
    );
    let err = report.into_result().unwrap_err();
    assert!(format!("{err:#}").contains("triage exploded"));
    let run = &ctx
        .db
        .list_pipeline_runs(&PipelineRunFilter::default(), 1)
        .unwrap()[0];
    assert_eq!(run.status, "failed");
    assert!(run.error.as_deref().unwrap().contains("triage"));
}

fn custom(src: &str) -> PipelineTemplate {
    let t: PipelineTemplate = toml::from_str(src).unwrap();
    template::validate_template(&t).unwrap();
    t
}

#[tokio::test]
async fn budget_timeout_and_scope() {
    // The budget is reached after the first triage: the second is skipped.
    let world = Arc::new(World {
        found: true,
        ai_calls: 1,
        ..Default::default()
    });
    let t = custom(
        r#"
name = "b"
max_ai_calls = 1
[[steps]]
id = "t1"
uses = "triage"
[[steps]]
id = "t2"
uses = "triage"
[[steps]]
id = "pr-only"
uses = "analyze"
[[steps]]
id = "after"
uses = "memory"
needs = ["pr-only"]
"#,
    );
    let ctx = ctx_with(ItemKind::Issue, Some(1), RepoFeatures::default());
    let report = execute(&t, &fake_registry(&world), &ctx).await;
    let ids: Vec<&str> = report.steps.iter().map(|s| s.step_id.as_str()).collect();
    assert_eq!(ids, ["t1", "t2", "after"], "analyze is out of an issue run");
    assert_eq!(
        report.steps[1].reason.as_deref(),
        Some("AI call budget of 1 reached")
    );
    assert_eq!(
        report.steps[2].reason.as_deref(),
        Some("needs 'pr-only', which does not run on issue #1")
    );

    // Timeout: failed with the reason, and continue goes on.
    let world = Arc::new(World {
        found: true,
        sleep_ms: 1500,
        ..Default::default()
    });
    let t = custom(
        r#"
name = "t"
[[steps]]
id = "slow"
uses = "sync"
timeout_secs = 1
on_failure = "continue"
[[steps]]
id = "next"
uses = "memory"
"#,
    );
    let ctx = ctx_with(ItemKind::Issue, Some(1), RepoFeatures::default());
    let report = execute(&t, &fake_registry(&world), &ctx).await;
    assert_eq!(report.steps[0].status, StepStatus::Failed);
    assert_eq!(
        report.steps[0].reason.as_deref(),
        Some("timed out after 1s")
    );
    assert_eq!(report.steps[1].status, StepStatus::Done);
    assert!(report.into_result().is_ok());
}

#[tokio::test]
async fn no_number_runs_but_records_nothing() {
    let world = Arc::new(World::default());
    let ctx = ctx_with(
        ItemKind::Issue,
        None,
        features(false, false, false, false, false),
    );
    let report = execute(template::standard(), &fake_registry(&world), &ctx).await;
    assert!(report.run_id.is_none());
    assert!(ctx
        .db
        .list_pipeline_runs(&PipelineRunFilter::default(), 10)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn routes_pick_the_template_and_invalid_falls_back() {
    let ctx = ctx_with(ItemKind::Issue, Some(3), RepoFeatures::default());
    seed_issue(&ctx, 3, &["docs"], "a");
    let reg = StepRegistry::builtin();
    assert_eq!(select_template(&ctx, &reg).await.name, "standard");

    let over = serde_json::json!({
        "pipelines": [{"name": "light", "steps": [{"id": "s", "uses": "sync"}]},
                      {"name": "broken", "steps": [{"id": "s", "uses": "nope"}]}],
        "pipeline_routes": [
            {"template": "broken", "when": {"labels_any": ["urgent"]}},
            {"template": "light", "when": {"labels_any": ["docs"]}}
        ]
    });
    ctx.db
        .set_app_setting(crate::db::settings::PIPELINES_KEY, &over.to_string())
        .unwrap();
    assert_eq!(select_template(&ctx, &reg).await.name, "light");

    seed_issue(&ctx, 3, &["urgent"], "a");
    assert_eq!(
        select_template(&ctx, &reg).await.name,
        "standard",
        "an unknown step kind falls back to standard"
    );
    // The API refuses that override up front.
    assert!(
        template::validate_override(&template::from_config(&ctx.config), &over, &reg)
            .unwrap_err()
            .contains("unknown step kind")
    );
}

#[tokio::test]
async fn ai_calls_outside_a_step_are_ignored() {
    note_ai_call(); // no task-local: must not panic
}

#[tokio::test]
async fn partial_run_follows_template_conditions() {
    // A custom template whose triage skips `skip-ai` issues: the scheduler
    // (partial run of the triage steps) must not triage them, and records
    // nothing when nothing ran.
    let t = custom(
        r#"
name = "careful"
[[steps]]
id = "sync"
uses = "sync"
[[steps]]
id = "triage"
uses = "triage"
needs = ["sync"]
when = { labels_none = ["skip-ai"], features = ["triage_issues"] }
"#,
    );
    let world = Arc::new(World::default());
    let opts = ExecOptions {
        only_kinds: Some(vec!["triage".into()]),
        record_only_if_active: true,
        ..Default::default()
    };
    let feats = features(true, true, false, false, false);
    let ctx = ctx_with(ItemKind::Issue, Some(8), feats.clone());
    seed_issue(&ctx, 8, &["skip-ai"], "a");
    let report = execute_with(&t, &fake_registry(&world), &ctx, &opts).await;
    assert!(
        world.log.lock().unwrap().is_empty(),
        "sync left out, triage skipped"
    );
    assert_eq!(report.steps.len(), 1);
    assert_eq!(
        report.steps[0].reason.as_deref(),
        Some("label 'skip-ai' on issue #8")
    );
    assert!(
        report.run_id.is_none(),
        "all-skipped partial run not recorded"
    );

    // Without the label, the triage runs; `needs = [sync]` counts as met.
    let ctx = ctx_with(ItemKind::Issue, Some(9), feats);
    seed_issue(&ctx, 9, &[], "a");
    let report = execute_with(&t, &fake_registry(&world), &ctx, &opts).await;
    assert_eq!(*world.log.lock().unwrap(), ["triage"]);
    let run = &ctx
        .db
        .list_pipeline_runs(&PipelineRunFilter::default(), 1)
        .unwrap()[0];
    assert_eq!(run.id, report.run_id.unwrap());
    assert_eq!(run.plan.len(), 1);
    assert_eq!(run.steps[0].status, "done");
}

#[tokio::test]
async fn command_runs_ignore_the_toggles_only() {
    let t = custom(
        r#"
name = "c"
[[steps]]
id = "triage"
uses = "triage"
when = { features = ["triage_issues"], repo_filters = true, labels_none = ["frozen"] }
"#,
    );
    let world = Arc::new(World::default());
    let mut feats = features(true, false, false, false, true);
    feats.filters.skip_authors = vec!["bot".into()];
    let opts = ExecOptions {
        only_kinds: Some(vec!["triage".into()]),
        ignore_toggles: true,
        ..Default::default()
    };
    let ctx = ctx_with(ItemKind::Issue, Some(1), feats.clone());
    seed_issue(&ctx, 1, &[], "bot");
    let report = execute_with(&t, &fake_registry(&world), &ctx, &opts).await;
    assert_eq!(
        report.status("triage"),
        Some(StepStatus::Done),
        "toggle + filter ignored"
    );
    let ctx = ctx_with(ItemKind::Issue, Some(2), feats);
    seed_issue(&ctx, 2, &["frozen"], "bot");
    let report = execute_with(&t, &fake_registry(&world), &ctx, &opts).await;
    assert_eq!(
        report.status("triage"),
        Some(StepStatus::Skipped),
        "labels still apply"
    );
    assert!(report.run_id.is_some(), "command runs are always recorded");
}

#[tokio::test]
async fn background_ai_calls_are_added_to_the_run() {
    let world = Arc::new(World {
        found: true,
        ai_calls: 1,
        ..Default::default()
    });
    let t = custom(
        r#"
name = "bg"
max_ai_calls = 5
[[steps]]
id = "analyze"
uses = "analyze"
"#,
    );
    let ctx = ctx_with(ItemKind::Pr, Some(3), RepoFeatures::default());
    let report = execute(&t, &fake_registry(&world), &ctx).await;
    assert_eq!(
        *world.budget_seen.lock().unwrap(),
        Some(Some(4)),
        "budget left counts the running step"
    );
    // The diagram job finishes after the run.
    let sink = world
        .sink
        .lock()
        .unwrap()
        .take()
        .expect("recorded run gives a sink");
    let (_, n) = count_ai_calls(async {
        note_ai_call();
        note_ai_call();
    })
    .await;
    sink(n);
    let run = &ctx
        .db
        .list_pipeline_runs(&PipelineRunFilter::default(), 1)
        .unwrap()[0];
    assert_eq!(run.id, report.run_id.unwrap());
    assert_eq!(run.ai_calls, 3);
    assert_eq!(run.steps[0].ai_calls, 3);
}

#[tokio::test]
async fn routes_use_the_payload_labels_before_sync() {
    let ctx = ctx_with(ItemKind::Pr, Some(4), RepoFeatures::default());
    seed_pull(&ctx, 4, "a", "main"); // cached without the new label
    let payload = serde_json::json!({
        "action": "labeled",
        "pull_request": {"number": 4, "labels": [{"name": "docs"}], "user": {"login": "a"}, "head": {"sha": "h1"}}
    });
    let info = ItemInfo::from_payload(ItemKind::Pr, &payload).unwrap();
    assert_eq!(info.labels, ["docs"]);
    assert_eq!(info.head_sha.as_deref(), Some("h1"));
    let ctx = ctx.with_payload_info(Some(info));
    let over = serde_json::json!({
        "pipelines": [{"name": "light", "steps": [{"id": "s", "uses": "sync"}]}],
        "pipeline_routes": [{"template": "light", "when": {"labels_any": ["docs"]}}]
    });
    ctx.db
        .set_app_setting(crate::db::settings::PIPELINES_KEY, &over.to_string())
        .unwrap();
    assert_eq!(
        select_template(&ctx, &StepRegistry::builtin()).await.name,
        "light"
    );
    // Poller payloads carry no labels: no payload info.
    assert!(ItemInfo::from_payload(
        ItemKind::Pr,
        &serde_json::json!({"pull_request": {"number": 4}})
    )
    .is_none());
    // Comment payloads carry the issue object.
    let c = serde_json::json!({"issue": {"labels": [{"name": "x"}], "pull_request": {}}});
    assert_eq!(
        ItemInfo::from_payload(ItemKind::Pr, &c).unwrap().labels,
        ["x"]
    );
}

#[tokio::test]
async fn soft_failure_is_recorded_without_abort() {
    struct Soft;
    #[async_trait]
    impl Step for Soft {
        fn kind(&self) -> &'static str {
            "triage"
        }
        fn items(&self) -> &'static [ItemKind] {
            &[ItemKind::Issue]
        }
        async fn run(&self, _ctx: &RunContext, _def: &StepDef) -> anyhow::Result<StepOutput> {
            Ok(StepOutput::soft_failed("AI said no"))
        }
    }
    let world = Arc::new(World {
        found: true,
        ..Default::default()
    });
    let mut reg = fake_registry(&world);
    reg.register(Arc::new(Soft));
    let ctx = ctx_with(
        ItemKind::Issue,
        Some(5),
        features(true, true, false, false, false),
    );
    let report = execute(template::standard(), &reg, &ctx).await;
    assert_eq!(report.status("triage"), Some(StepStatus::Failed));
    assert_eq!(report.steps[1].reason.as_deref(), Some("AI said no"));
    assert_eq!(
        report.status("triage-memory"),
        Some(StepStatus::Skipped),
        "needs a done triage"
    );
    assert!(report.into_result().is_ok(), "event not failed, as before");
}
