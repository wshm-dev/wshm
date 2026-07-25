//! PR subject hierarchy for the network graph.
//!
//! Over ALL pull requests in the DB (open + closed), rank the "grand groupes" —
//! the most frequent subjects across PR titles — and, within each group's PRs,
//! rank the frequent secondary terms as "sous-groupes", each carrying the PRs
//! it groups. Titles only, DB-wide, deterministic; same tokenisation as domain
//! discovery (singularised, repo-name excluded), so groups line up with the
//! discovered domains.

use serde::Serialize;
use std::collections::HashSet;

use crate::config::Config;
use crate::db::backend::DatabaseBackend;
use crate::pipelines::discover_domains::{singular, title_words, top_terms};

/// Effective "all PRs" cap (title-only, so cheap even in the tens of thousands).
const CLOSED_CAP: usize = 100_000;
/// Cap the PR list carried by each subgroup so the payload stays bounded on
/// very large groups; the `count` is always the true total.
const PR_CAP_PER_SUBGROUP: usize = 300;

/// A pull request reference (enough for the graph's PR list on click).
#[derive(Serialize)]
pub struct PrRef {
    pub number: u64,
    pub title: String,
}

/// A secondary subject inside a grand groupe.
#[derive(Serialize)]
pub struct SubGroup {
    pub name: String,
    /// Distinct PRs in the parent group whose title mentions this term.
    pub count: usize,
    /// Up to `PR_CAP_PER_SUBGROUP` of those PRs.
    pub prs: Vec<PrRef>,
}

/// A grand groupe (top-level subject) and its subgroups.
#[derive(Serialize)]
pub struct Group {
    pub name: String,
    /// Distinct PRs whose title mentions this subject.
    pub count: usize,
    pub subgroups: Vec<SubGroup>,
}

/// The repo's own owner/name — self-referential noise, excluded from ranking.
fn repo_stop(config: &Config) -> HashSet<String> {
    let mut s = HashSet::new();
    for part in config.repo_slug().to_lowercase().split('/') {
        if part.len() >= 2 {
            s.insert(part.to_string());
            s.insert(singular(part));
        }
    }
    s
}

/// Build the group → subgroup → PRs hierarchy across every PR in the DB.
/// `group_limit` = number of grand groupes, `sub_limit` = subgroups per group.
pub fn build(
    config: &Config,
    db: &dyn DatabaseBackend,
    group_limit: usize,
    sub_limit: usize,
) -> Vec<Group> {
    // (number, title, singularised word-set) for every PR — open + all closed.
    let mut prs: Vec<(u64, String, HashSet<String>)> = Vec::new();
    for p in db.get_open_pulls().unwrap_or_default() {
        let w = title_words(&p.title);
        prs.push((p.number, p.title, w));
    }
    for p in db.get_closed_pulls(CLOSED_CAP).unwrap_or_default() {
        let w = title_words(&p.title);
        prs.push((p.number, p.title, w));
    }

    let all_titles: Vec<String> = prs.iter().map(|(_, t, _)| t.clone()).collect();
    let stop = repo_stop(config);

    // Grand groupes: the top subjects across all PR titles.
    let group_terms = top_terms(&all_titles, &[], &stop, group_limit);

    let mut groups = Vec::with_capacity(group_terms.len());
    for (g, _) in group_terms {
        let members: Vec<&(u64, String, HashSet<String>)> =
            prs.iter().filter(|(_, _, w)| w.contains(&g)).collect();
        let g_count = members.len();

        // Sous-groupes: frequent secondary terms among this group's PRs,
        // excluding the group term itself and the repo name.
        let member_titles: Vec<String> = members.iter().map(|(_, t, _)| t.clone()).collect();
        let mut sub_stop = stop.clone();
        sub_stop.insert(g.clone());
        let sub_terms = top_terms(&member_titles, &[], &sub_stop, sub_limit);

        let mut subgroups = Vec::with_capacity(sub_terms.len());
        for (s, _) in sub_terms {
            let mut list = Vec::new();
            let mut count = 0usize;
            for (num, title, w) in members.iter().copied() {
                if w.contains(&s) {
                    count += 1;
                    if list.len() < PR_CAP_PER_SUBGROUP {
                        list.push(PrRef {
                            number: *num,
                            title: title.clone(),
                        });
                    }
                }
            }
            subgroups.push(SubGroup {
                name: s,
                count,
                prs: list,
            });
        }

        groups.push(Group {
            name: g,
            count: g_count,
            subgroups,
        });
    }
    groups
}
