//! Publication of the interactive page (`[pr.review_diagrams.publish]`).

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use crate::config::{DiagramPublishConfig, DiagramPublishTarget};
use crate::db::diagrams::PrDiagramRow;
use crate::github::Client as GhClient;

/// Branch of `pages_repo` the pages are committed to.
pub const PAGES_BRANCH: &str = "gh-pages";
/// Pro feature gating the daemon-hosted pages (`target = "wshm"`).
pub const HOSTING_FEATURE: &str = "diagram_hosting";

pub fn target_name(t: DiagramPublishTarget) -> &'static str {
    match t {
        DiagramPublishTarget::Off => "off",
        DiagramPublishTarget::Local => "local",
        DiagramPublishTarget::GithubPages => "github-pages",
        DiagramPublishTarget::Wshm => "wshm",
        DiagramPublishTarget::S3 => "s3",
        DiagramPublishTarget::Gcs => "gcs",
        DiagramPublishTarget::Azure => "azure",
    }
}

/// Where a page ended up.
#[derive(Debug, Clone, PartialEq)]
pub struct Published {
    pub target: &'static str,
    pub key: String,
    pub url: Option<String>,
    pub token: Option<String>,
    /// HTML to keep in the DB (only for the `wshm` target).
    pub html: Option<String>,
}

/// Does this target make the page reachable by anyone with the link?
pub fn is_public_target(cfg: &DiagramPublishConfig) -> bool {
    match cfg.target {
        DiagramPublishTarget::Off | DiagramPublishTarget::Wshm => false,
        DiagramPublishTarget::Local => cfg.public_url.is_some(),
        _ => true,
    }
}

fn random_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// One path segment: `[A-Za-z0-9._-]`, never `.` or `..`.
fn segment(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('.').to_string();
    if s.is_empty() {
        "x".into()
    } else {
        s
    }
}

/// Object key of a page: `prefix/owner/repo/pr-N-<token>.html` (the token
/// keeps pages from being enumerated on a public bucket or Pages site).
pub fn page_key(prefix: &str, repo_slug: &str, pr: u64, token: &str) -> String {
    let mut parts: Vec<String> = prefix
        .split('/')
        .filter(|p| !p.trim().is_empty())
        .map(segment)
        .collect();
    parts.extend(repo_slug.split('/').map(segment));
    parts.push(format!("pr-{pr}-{}.html", segment(token)));
    parts.join("/")
}

fn join_url(base: &str, key: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        key.trim_start_matches('/')
    )
}

/// Default Pages URL of `owner/name`.
fn pages_url(pages_repo: &str) -> Option<String> {
    let (owner, name) = pages_repo.split_once('/')?;
    let host = format!("{}.github.io", owner.to_ascii_lowercase());
    Some(if name.eq_ignore_ascii_case(&host) {
        format!("https://{host}")
    } else {
        format!("https://{host}/{name}")
    })
}

fn valid_repo_full(s: &str) -> bool {
    let mut it = s.split('/');
    let ok = |p: Option<&str>| {
        p.is_some_and(|p| {
            !p.is_empty()
                && p != "."
                && p != ".."
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    };
    ok(it.next()) && ok(it.next()) && it.next().is_none()
}

/// Publish `html`. `Ok(None)` when the target is off, not available, refused
/// (private repo) or skipped in dry-run; every such case is logged.
/// `previous` lets a republish keep the same key/token, hence the same link.
#[allow(clippy::too_many_arguments)]
pub async fn publish(
    cfg: &DiagramPublishConfig,
    gh: &GhClient,
    repo_slug: &str,
    pr: u64,
    html: &str,
    is_private: bool,
    previous: Option<&PrDiagramRow>,
    dry_run: bool,
) -> Result<Option<Published>> {
    let name = target_name(cfg.target);
    match cfg.target {
        DiagramPublishTarget::Off => return Ok(None),
        DiagramPublishTarget::S3 | DiagramPublishTarget::Gcs | DiagramPublishTarget::Azure => {
            warn!(
                "PR diagram page: target '{name}' requires object storage support (pending) — page not published"
            );
            return Ok(None);
        }
        _ => {}
    }
    if is_private && is_public_target(cfg) && !cfg.allow_private {
        warn!(
            "PR diagram page: {repo_slug} is private (or its visibility is unknown) and target '{name}' is public — not published (set publish.allow_private = true to override)"
        );
        return Ok(None);
    }
    if dry_run && cfg.target != DiagramPublishTarget::Local {
        info!("PR diagram page: dry-run, not published to '{name}'");
        return Ok(None);
    }

    let same_target = previous.and_then(|p| p.page_target.as_deref()) == Some(name);
    let token = previous
        .filter(|_| same_target)
        .and_then(|p| p.page_token.clone())
        .unwrap_or_else(random_token);
    let key = previous
        .filter(|_| same_target)
        .and_then(|p| p.page_key.clone())
        .unwrap_or_else(|| page_key(&cfg.prefix, repo_slug, pr, &token));

    match cfg.target {
        DiagramPublishTarget::Local => {
            let dir = cfg
                .dir
                .as_deref()
                .filter(|d| !d.trim().is_empty())
                .context("publish.dir is required for target = \"local\"")?;
            let path = std::path::Path::new(dir).join(&key);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&path, html).with_context(|| format!("writing {}", path.display()))?;
            info!("PR diagram page written to {}", path.display());
            Ok(Some(Published {
                target: name,
                url: cfg.public_url.as_deref().map(|u| join_url(u, &key)),
                key,
                token: Some(token),
                html: None,
            }))
        }
        DiagramPublishTarget::GithubPages => {
            let repo = cfg
                .pages_repo
                .as_deref()
                .filter(|r| valid_repo_full(r))
                .context(
                    "publish.pages_repo (owner/repo) is required for target = \"github-pages\"",
                )?;
            gh.put_file_content(
                repo,
                PAGES_BRANCH,
                &key,
                html.as_bytes(),
                &format!("wshm: architecture page for {repo_slug}#{pr}"),
            )
            .await?;
            let base = cfg.public_url.clone().or_else(|| pages_url(repo));
            info!("PR diagram page committed to {repo}@{PAGES_BRANCH}:{key}");
            Ok(Some(Published {
                target: name,
                url: base.map(|b| join_url(&b, &key)),
                key,
                token: Some(token),
                html: None,
            }))
        }
        DiagramPublishTarget::Wshm => {
            if !crate::pro_hooks::has_feature(HOSTING_FEATURE) {
                warn!(
                    "PR diagram page: target 'wshm' needs the Pro feature '{HOSTING_FEATURE}' — page not published"
                );
                return Ok(None);
            }
            let base = cfg
                .public_url
                .as_deref()
                .filter(|u| !u.trim().is_empty())
                .context(
                "publish.public_url (the daemon's external URL) is required for target = \"wshm\"",
            )?;
            let (owner, name_) = repo_slug.split_once('/').unwrap_or((repo_slug, ""));
            Ok(Some(Published {
                target: name,
                url: Some(format!(
                    "{}/diagrams/{owner}/{name_}/{pr}?k={token}",
                    base.trim_end_matches('/')
                )),
                key: format!("db:{repo_slug}#{pr}"),
                token: Some(token),
                html: Some(html.to_string()),
            }))
        }
        _ => bail!("unreachable publish target {name}"),
    }
}

/// Remove a published page (best effort for remote targets). The DB row is
/// the caller's business.
pub async fn delete_page(
    cfg: &DiagramPublishConfig,
    gh: &GhClient,
    row: &PrDiagramRow,
) -> Result<()> {
    let (Some(target), Some(key)) = (row.page_target.as_deref(), row.page_key.as_deref()) else {
        return Ok(());
    };
    match target {
        "local" => {
            let Some(dir) = cfg.dir.as_deref() else {
                warn!("PR diagram page {key}: publish.dir no longer set, local file left in place");
                return Ok(());
            };
            if key.split('/').any(|s| s == ".." || s.is_empty()) {
                bail!("refusing to delete suspicious key {key}");
            }
            match std::fs::remove_file(std::path::Path::new(dir).join(key)) {
                Ok(()) => info!("Deleted PR diagram page {dir}/{key}"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("deleting local diagram page"),
            }
        }
        "github-pages" => {
            let Some(repo) = cfg.pages_repo.as_deref().filter(|r| valid_repo_full(r)) else {
                warn!(
                    "PR diagram page {key}: publish.pages_repo no longer set, page left in place"
                );
                return Ok(());
            };
            if gh
                .delete_file_content(repo, PAGES_BRANCH, key, "wshm: remove architecture page")
                .await?
            {
                info!("Deleted PR diagram page {repo}@{PAGES_BRANCH}:{key}");
            }
        }
        // `wshm`: the HTML lives in the row itself.
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_sanitized_paths() {
        assert_eq!(
            page_key("wshm-diagrams", "rtk-ai/rtk", 4544, "ab12"),
            "wshm-diagrams/rtk-ai/rtk/pr-4544-ab12.html"
        );
        assert_eq!(
            page_key("../../etc/ /x y", "o/../r", 1, "t"),
            "x/x/etc/x-y/o/x/r/pr-1-t.html"
        );
    }

    #[test]
    fn pages_url_handles_user_sites() {
        assert_eq!(
            pages_url("Acme/diagrams").unwrap(),
            "https://acme.github.io/diagrams"
        );
        assert_eq!(
            pages_url("acme/acme.github.io").unwrap(),
            "https://acme.github.io"
        );
        assert!(pages_url("nope").is_none());
    }

    #[test]
    fn public_targets() {
        let mut c = DiagramPublishConfig::default();
        assert!(!is_public_target(&c));
        c.target = DiagramPublishTarget::Local;
        assert!(!is_public_target(&c));
        c.public_url = Some("https://x".into());
        assert!(is_public_target(&c));
        c.target = DiagramPublishTarget::GithubPages;
        assert!(is_public_target(&c));
        c.target = DiagramPublishTarget::Wshm;
        assert!(
            !is_public_target(&c),
            "wshm guards private repos with a session"
        );
    }

    #[test]
    fn repo_full_validation() {
        assert!(valid_repo_full("a/b.c"));
        assert!(!valid_repo_full("a"));
        assert!(!valid_repo_full("a/b/c"));
        assert!(!valid_repo_full("../b"));
    }
}
