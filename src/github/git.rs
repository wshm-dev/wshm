use anyhow::{Context, Result};
use std::path::Path;
use tracing::info;

/// Clone a repository to a local path using secure credential callbacks.
pub fn clone_repo(url: &str, path: &Path, token: &str) -> Result<git2::Repository> {
    info!("Cloning {url} to {}", path.display());

    let token = token.to_string();
    let mut cb = git2::RemoteCallbacks::new();
    cb.credentials(move |_url, _username, _allowed| {
        git2::Cred::userpass_plaintext("x-access-token", &token)
    });

    let mut fo = git2::FetchOptions::new();
    fo.remote_callbacks(cb);

    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fo);

    let repo = builder
        .clone(url, path)
        .with_context(|| format!("Failed to clone {url}"))?;

    Ok(repo)
}

/// Check if a branch has merge conflicts with the base branch.
pub fn has_conflicts(repo: &git2::Repository, head_ref: &str, base_ref: &str) -> Result<bool> {
    let head = repo
        .find_branch(head_ref, git2::BranchType::Local)
        .or_else(|_| repo.find_branch(&format!("origin/{head_ref}"), git2::BranchType::Remote))
        .context("Failed to find head branch")?;

    let base = repo
        .find_branch(base_ref, git2::BranchType::Local)
        .or_else(|_| repo.find_branch(&format!("origin/{base_ref}"), git2::BranchType::Remote))
        .context("Failed to find base branch")?;

    let head_commit = head.get().peel_to_commit()?;
    let base_commit = base.get().peel_to_commit()?;

    let ancestor = repo.merge_base(head_commit.id(), base_commit.id())?;
    let ancestor_commit = repo.find_commit(ancestor)?;

    let head_tree = head_commit.tree()?;
    let base_tree = base_commit.tree()?;
    let ancestor_tree = ancestor_commit.tree()?;

    let index = repo.merge_trees(&ancestor_tree, &base_tree, &head_tree, None)?;

    Ok(index.has_conflicts())
}
