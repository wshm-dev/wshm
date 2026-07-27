//! PR subject hierarchy for the network graph.
//!
//! Over a set of pull requests (open + closed, one repo or all), rank the
//! "grand groupes" — the most frequent subjects across PR titles — and, within
//! each group's PRs, rank the frequent secondary terms as "sous-groupes", each
//! carrying the PRs it groups. Titles only, deterministic; same tokenisation as
//! domain discovery (singularised, repo-name excluded), so groups line up with
//! the discovered domains.

use serde::Serialize;
use std::collections::HashSet;

use crate::pipelines::discover_domains::{title_words, top_terms};

/// Cap the PR list carried by each subgroup so the payload stays bounded on
/// very large groups; the `count` is always the true total.
const PR_CAP_PER_SUBGROUP: usize = 200;

/// A pull request (or issue) referenced by a group/subgroup. Carries its repo
/// so the UI can link correctly across repos and for closed items (which the
/// in-app PR view doesn't load).
#[derive(Serialize)]
pub struct PrRef {
    pub number: u64,
    pub title: String,
    pub repo: String,
}

/// A secondary subject inside a grand groupe.
#[derive(Serialize)]
pub struct SubGroup {
    pub name: String,
    pub count: usize,
    pub prs: Vec<PrRef>,
}

/// A grand groupe (top-level subject), its subgroups, and a sample of its own
/// PRs (so clicking the group can list them, not just its subgroups).
#[derive(Serialize)]
pub struct Group {
    pub name: String,
    pub count: usize,
    pub subgroups: Vec<SubGroup>,
    pub prs: Vec<PrRef>,
}

/// Build the group → subgroup → PRs hierarchy from a PR set.
///
/// `prs` is (number, title, repo) for every PR to consider; `name_stop` holds the
/// repo owner/name tokens to exclude as self-referential noise. `group_limit` =
/// number of grand groupes, `sub_limit` = subgroups per group.
pub fn build_from(
    prs: &[(u64, String, String)],
    name_stop: &HashSet<String>,
    group_limit: usize,
    sub_limit: usize,
) -> Vec<Group> {
    // (number, title, repo, singularised word-set) per PR.
    let items: Vec<(u64, String, String, HashSet<String>)> = prs
        .iter()
        .map(|(n, t, r)| (*n, t.clone(), r.clone(), title_words(t)))
        .collect();
    let all_titles: Vec<String> = items.iter().map(|(_, t, _, _)| t.clone()).collect();

    // Grand groupes: the top subjects across all PR titles.
    let group_terms = top_terms(&all_titles, &[], name_stop, group_limit);

    let mut groups = Vec::with_capacity(group_terms.len());
    for (g, _) in group_terms {
        let members: Vec<&(u64, String, String, HashSet<String>)> =
            items.iter().filter(|(_, _, _, w)| w.contains(&g)).collect();
        let g_count = members.len();

        // Sous-groupes: frequent secondary terms among this group's PRs,
        // excluding the group term itself and the repo name.
        let member_titles: Vec<String> = members.iter().map(|(_, t, _, _)| t.clone()).collect();
        let mut sub_stop = name_stop.clone();
        sub_stop.insert(g.clone());
        let sub_terms = top_terms(&member_titles, &[], &sub_stop, sub_limit);

        let mut subgroups = Vec::with_capacity(sub_terms.len());
        for (s, _) in sub_terms {
            let mut count = 0usize;
            let mut list: Vec<PrRef> = Vec::new();
            for (num, title, repo, w) in members.iter().copied() {
                if w.contains(&s) {
                    count += 1;
                    if list.len() < PR_CAP_PER_SUBGROUP {
                        list.push(PrRef {
                            number: *num,
                            title: title.clone(),
                            repo: repo.clone(),
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

        // The group's own PRs (capped), for the "click a group" list.
        let group_prs: Vec<PrRef> = members
            .iter()
            .take(PR_CAP_PER_SUBGROUP)
            .map(|(num, title, repo, _)| PrRef {
                number: *num,
                title: title.clone(),
                repo: repo.clone(),
            })
            .collect();

        groups.push(Group {
            name: g,
            count: g_count,
            subgroups,
            prs: group_prs,
        });
    }
    groups
}
