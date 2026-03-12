use wshm::db::Database;
use wshm::db::issues::Issue;
use wshm::db::pulls::PullRequest;

fn sample_issue() -> Issue {
    Issue {
        number: 1,
        title: "Test issue".into(),
        body: Some("Body text".into()),
        state: "open".into(),
        labels: vec!["bug".into()],
        author: Some("user".into()),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn sample_pr() -> PullRequest {
    PullRequest {
        number: 10,
        title: "Fix bug".into(),
        body: Some("Fixes #1".into()),
        state: "open".into(),
        labels: vec![],
        author: Some("dev".into()),
        head_sha: Some("abc123".into()),
        base_sha: Some("def456".into()),
        head_ref: Some("fix/bug".into()),
        base_ref: Some("main".into()),
        mergeable: Some(true),
        ci_status: Some("success".into()),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn test_issue_crud() {
    let db = Database::open_memory().unwrap();
    let issue = sample_issue();

    db.upsert_issue(&issue).unwrap();

    let fetched = db.get_issue(1).unwrap().unwrap();
    assert_eq!(fetched.title, "Test issue");
    assert_eq!(fetched.labels, vec!["bug".to_string()]);

    let open = db.get_open_issues().unwrap();
    assert_eq!(open.len(), 1);

    let untriaged = db.get_untriaged_issues().unwrap();
    assert_eq!(untriaged.len(), 1);
}

#[test]
fn test_pr_crud() {
    let db = Database::open_memory().unwrap();
    let pr = sample_pr();

    db.upsert_pull(&pr).unwrap();

    let fetched = db.get_pull(10).unwrap().unwrap();
    assert_eq!(fetched.title, "Fix bug");
    assert_eq!(fetched.mergeable, Some(true));

    let open = db.get_open_pulls().unwrap();
    assert_eq!(open.len(), 1);
}

#[test]
fn test_sync_log() {
    let db = Database::open_memory().unwrap();

    assert!(db.get_sync_entry("issues").unwrap().is_none());

    db.update_sync_entry("issues", "2026-01-01T00:00:00Z", Some("etag123"))
        .unwrap();

    let entry = db.get_sync_entry("issues").unwrap().unwrap();
    assert_eq!(entry.last_synced_at, "2026-01-01T00:00:00Z");
    assert_eq!(entry.etag.as_deref(), Some("etag123"));
}

#[test]
fn test_triage_result() {
    let db = Database::open_memory().unwrap();
    let issue = sample_issue();
    db.upsert_issue(&issue).unwrap();

    assert!(!db.is_triaged(1).unwrap());

    let classification = wshm::ai::schemas::IssueClassification {
        category: "bug".into(),
        confidence: 0.92,
        priority: Some("high".into()),
        summary: "Memory leak".into(),
        suggested_labels: vec!["bug".into()],
        is_duplicate_of: None,
        is_simple_fix: true,
        relevant_files: vec!["src/main.rs".into()],
    };

    db.upsert_triage_result(&classification, 1).unwrap();
    assert!(db.is_triaged(1).unwrap());

    // Untriaged should now be empty
    let untriaged = db.get_untriaged_issues().unwrap();
    assert_eq!(untriaged.len(), 0);
}
