// Project:   hyperi-rustlib
// File:      src/directory_config/git.rs
// Purpose:   Git operations for DirectoryConfigStore via git2
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Git integration for DirectoryConfigStore.
//!
//! Uses `git2` (libgit2 bindings) for native git operations without
//! requiring a system `git` binary. Feature-gated behind `directory-config-git`.

use std::path::Path;

use git2::{BranchType, Repository, Signature};

use crate::directory_config::error::{DirectoryConfigError, DirectoryConfigResult};

/// Open the git repository at the given directory.
fn open_repo(dir: &Path) -> DirectoryConfigResult<Repository> {
    Repository::open(dir)
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to open repository: {e}")))
}

/// Stage a file and commit with the given message.
///
/// Returns the short commit hash on success.
pub fn git_add_and_commit(
    dir: &Path,
    file: &str,
    message: &str,
    author_name: &str,
    author_email: &str,
) -> DirectoryConfigResult<String> {
    let repo = open_repo(dir)?;

    // Stage the file
    let mut index = repo
        .index()
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to get index: {e}")))?;
    index
        .add_path(Path::new(file))
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to stage file: {e}")))?;
    index
        .write()
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to write index: {e}")))?;
    let tree_oid = index
        .write_tree()
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to write tree: {e}")))?;

    let tree = repo
        .find_tree(tree_oid)
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to find tree: {e}")))?;

    let sig = Signature::now(author_name, author_email)
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to create signature: {e}")))?;

    // Get parent commit (HEAD), if any
    let parent_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit<'_>> = parent_commit.iter().collect();

    let commit_oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to create commit: {e}")))?;

    // Return short hash (first 7 chars)
    let hash = format!("{commit_oid}");
    let short = &hash[..hash.len().min(7)];
    Ok(short.to_string())
}

/// Push current branch to remote.
pub fn git_push(dir: &Path) -> DirectoryConfigResult<()> {
    let repo = open_repo(dir)?;

    let head = repo
        .head()
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to get HEAD: {e}")))?;
    let refname = head
        .name()
        .ok_or_else(|| DirectoryConfigError::GitError("HEAD is not a valid UTF-8 ref".into()))?;

    let mut remote = repo
        .find_remote("origin")
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to find remote: {e}")))?;

    remote
        .push(&[refname], None)
        .map_err(|e| DirectoryConfigError::GitError(format!("git push failed: {e}")))?;

    Ok(())
}

/// Get the current branch name.
#[must_use]
pub fn git_current_branch(dir: &Path) -> Option<String> {
    let repo = Repository::open(dir).ok()?;
    let head = repo.head().ok()?;
    if head.is_branch() {
        head.shorthand().map(String::from)
    } else {
        None
    }
}

/// List all local branches.
pub fn git_list_branches(dir: &Path) -> DirectoryConfigResult<Vec<String>> {
    let repo = open_repo(dir)?;

    let branches = repo
        .branches(Some(BranchType::Local))
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to list branches: {e}")))?;

    let mut names = Vec::new();
    for branch_result in branches {
        let (branch, _) = branch_result
            .map_err(|e| DirectoryConfigError::GitError(format!("branch iteration: {e}")))?;
        if let Some(name) = branch.name().ok().flatten() {
            names.push(name.to_string());
        }
    }

    Ok(names)
}

/// Switch to a branch, optionally creating it.
pub fn git_switch_branch(dir: &Path, branch: &str, create: bool) -> DirectoryConfigResult<()> {
    let repo = open_repo(dir)?;

    if create {
        // Create branch at HEAD
        let head_commit = repo.head().and_then(|h| h.peel_to_commit()).map_err(|e| {
            DirectoryConfigError::GitError(format!("failed to get HEAD commit: {e}"))
        })?;
        repo.branch(branch, &head_commit, false)
            .map_err(|e| DirectoryConfigError::GitError(format!("failed to create branch: {e}")))?;
    }

    // Point HEAD to the branch
    let refname = format!("refs/heads/{branch}");
    repo.set_head(&refname)
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to set HEAD: {e}")))?;

    // Update working directory to match
    repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
        .map_err(|e| DirectoryConfigError::GitError(format!("failed to checkout: {e}")))?;

    Ok(())
}
