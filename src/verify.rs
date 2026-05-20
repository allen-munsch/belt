//! Ribbon verify — validate commit hashes against git origins.
//!
//! Ensures that OUTBOX claims (committed/completed events with commit hashes)
//! actually exist on the remote origin. Prevents agents from claiming work
//! that wasn't pushed.

use crate::event::{EventType, RibbonEvent};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of verifying a single commit hash.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifyResult {
    pub agent: String,
    pub commit: String,
    /// The task this commit is associated with.
    pub task: Option<String>,
    /// Whether the commit exists on origin/main.
    pub on_origin: bool,
    /// Error message if verification failed.
    pub error: Option<String>,
}

/// Configuration for git verification.
#[derive(Debug, Clone)]
pub struct GitRoots {
    /// Map of agent name → path to git repository root.
    pub roots: HashMap<String, PathBuf>,
    /// Remote name to check (default: "origin").
    pub remote: String,
    /// Branch to check (default: "main").
    pub branch: String,
}

impl Default for GitRoots {
    fn default() -> Self {
        GitRoots {
            roots: HashMap::new(),
            remote: "origin".to_string(),
            branch: "main".to_string(),
        }
    }
}

impl GitRoots {
    /// Add a git root for an agent.
    pub fn with_agent(mut self, agent: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        self.roots.insert(agent.into(), path.into());
        self
    }

    /// Set the remote name.
    pub fn with_remote(mut self, remote: impl Into<String>) -> Self {
        self.remote = remote.into();
        self
    }

    /// Set the branch name.
    pub fn with_branch(mut self, branch: impl Into<String>) -> Self {
        self.branch = branch.into();
        self
    }
}

/// Verify all commit-bearing events against their git origins.
///
/// For each event with a commit hash, checks if that commit exists on
/// the configured remote/branch. Events from agents without a configured
/// git root are skipped with a note.
pub fn verify_events(events: &[RibbonEvent], git_roots: &GitRoots) -> Vec<VerifyResult> {
    let mut results = Vec::new();

    // Collect unique (agent, commit) pairs
    let mut seen: HashMap<String, Vec<&RibbonEvent>> = HashMap::new();
    for event in events {
        if let Some(ref commit) = event.commit {
            if event.event_type == EventType::Committed || event.event_type == EventType::Completed
            {
                seen.entry(commit.clone()).or_default().push(event);
            }
        }
    }

    for (commit, commit_events) in &seen {
        let first = &commit_events[0];
        let agent = &first.agent;

        let result = match git_roots.roots.get(agent) {
            Some(root) => {
                match check_commit_on_origin(root, commit, &git_roots.remote, &git_roots.branch) {
                    Ok(true) => VerifyResult {
                        agent: agent.clone(),
                        commit: commit.clone(),
                        task: first.task.clone(),
                        on_origin: true,
                        error: None,
                    },
                    Ok(false) => VerifyResult {
                        agent: agent.clone(),
                        commit: commit.clone(),
                        task: first.task.clone(),
                        on_origin: false,
                        error: Some("Commit not found on origin — may not have been pushed".into()),
                    },
                    Err(e) => VerifyResult {
                        agent: agent.clone(),
                        commit: commit.clone(),
                        task: first.task.clone(),
                        on_origin: false,
                        error: Some(format!("Git error: {e}")),
                    },
                }
            }
            None => VerifyResult {
                agent: agent.clone(),
                commit: commit.clone(),
                task: first.task.clone(),
                on_origin: false,
                error: Some("No git root configured for this agent — cannot verify".into()),
            },
        };

        results.push(result);
    }

    // Sort by agent then commit
    results.sort_by(|a, b| a.agent.cmp(&b.agent).then(a.commit.cmp(&b.commit)));
    results
}

/// Check if a commit hash exists on origin/main.
///
/// Runs: `git fetch origin && git branch -r --contains <commit> origin/main`
fn check_commit_on_origin(
    repo_path: &Path,
    commit: &str,
    remote: &str,
    branch: &str,
) -> Result<bool, String> {
    // Fetch the remote first
    let fetch = Command::new("git")
        .args(["fetch", remote])
        .current_dir(repo_path)
        .output();

    if let Err(e) = fetch {
        return Err(format!("git fetch failed: {e}"));
    }

    // Check if commit is reachable from origin/branch
    let remote_branch = format!("{remote}/{branch}");
    let output = Command::new("git")
        .args(["branch", "-r", "--contains", commit, &remote_branch])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git branch failed: {e}"))?;

    if !output.status.success() {
        // The commit might not exist locally — try fetching it
        let fetch_commit = Command::new("git")
            .args(["fetch", remote, commit])
            .current_dir(repo_path)
            .output();

        if fetch_commit.is_err() {
            return Ok(false);
        }
    }

    // Re-check after possible fetch
    let output2 = Command::new("git")
        .args(["branch", "-r", "--contains", commit, &remote_branch])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git branch failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output2.stdout);
    Ok(stdout.contains(&remote_branch))
}

/// Print a human-readable verification report.
pub fn verify_report(results: &[VerifyResult]) -> String {
    if results.is_empty() {
        return "No commits to verify.\n".to_string();
    }

    let mut out = String::from("# Ribbon Verification Report\n\n");

    // Summary
    let total = results.len();
    let verified = results.iter().filter(|r| r.on_origin).count();
    let failed = total - verified;

    out.push_str(&format!(
        "**Total commits**: {total} | ✅ Verified: {verified} | ❌ Failed: {failed}\n\n"
    ));

    // Per-agent summary
    let mut by_agent: HashMap<&str, (usize, usize)> = HashMap::new();
    for r in results {
        let entry = by_agent.entry(&r.agent).or_insert((0, 0));
        entry.0 += 1;
        if r.on_origin {
            entry.1 += 1;
        }
    }

    out.push_str("| Agent | Commits | Verified | Status |\n");
    out.push_str("|-------|---------|----------|--------|\n");
    for (agent, (total, good)) in &by_agent {
        let status = if *good == *total {
            "✅ all"
        } else {
            "❌ missing"
        };
        out.push_str(&format!("| {agent} | {total} | {good} | {status} |\n"));
    }
    out.push('\n');

    // Details
    if failed > 0 {
        out.push_str("## ❌ Unverified Commits\n\n");
        for r in results.iter().filter(|r| !r.on_origin) {
            let short_hash = &r.commit[..r.commit.len().min(7)];
            out.push_str(&format!(
                "- **{agent}** `{hash}` — {task}: {error}\n",
                agent = r.agent,
                hash = short_hash,
                task = r.task.as_deref().unwrap_or("unknown task"),
                error = r.error.as_deref().unwrap_or("unknown error"),
            ));
        }
        out.push('\n');
    }

    if verified > 0 {
        out.push_str("## ✅ Verified Commits\n\n");
        for r in results.iter().filter(|r| r.on_origin) {
            let short_hash = &r.commit[..r.commit.len().min(7)];
            out.push_str(&format!(
                "- **{agent}** `{hash}` — {task}\n",
                agent = r.agent,
                hash = short_hash,
                task = r.task.as_deref().unwrap_or("unknown task"),
            ));
        }
        out.push('\n');
    }

    // Actionable hints
    if failed > 0 {
        let needs_config: Vec<_> = results
            .iter()
            .filter(|r| !r.on_origin && r.error.as_ref().is_some_and(|e| e.contains("No git root")))
            .map(|r| &r.agent)
            .collect();

        if !needs_config.is_empty() {
            out.push_str("## 🔧 How to fix\n\n");
            out.push_str("Add git roots to .ribbon/config.toml:\n\n```toml\n[git_roots]\n");
            for agent in &needs_config {
                out.push_str(&format!("{agent} = \"path/to/{agent}/repo\"\n"));
            }
            out.push_str("```\n\n");
            out.push_str("Or re-initialize with:\n\n```bash\n");
            let init_args: Vec<String> = needs_config
                .iter()
                .map(|a| format!("--git-root {a}=."))
                .collect();
            out.push_str(&format!("ribbon init {}\n```\n\n", init_args.join(" ")));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_no_git_roots() {
        let events = vec![RibbonEvent::new("mosaic", EventType::Completed)
            .with_commit("abc123def456")
            .with_task("some task")];

        let git_roots = GitRoots::default();
        let results = verify_events(&events, &git_roots);
        assert_eq!(results.len(), 1);
        assert!(!results[0].on_origin);
        assert!(results[0].error.as_ref().unwrap().contains("No git root"));
    }

    #[test]
    fn test_verify_with_nonexistent_path() {
        let events = vec![RibbonEvent::new("mosaic", EventType::Completed)
            .with_commit("abc123def456")
            .with_task("some task")];

        let git_roots = GitRoots::default().with_agent("mosaic", "/tmp/does-not-exist");

        let results = verify_events(&events, &git_roots);
        assert_eq!(results.len(), 1);
        assert!(!results[0].on_origin);
        // Should have a git error
    }

    #[test]
    fn test_verify_no_commit_events() {
        let events = vec![RibbonEvent::new("mosaic", EventType::Note).with_msg("just a note")];

        let git_roots = GitRoots::default();
        let results = verify_events(&events, &git_roots);
        assert!(results.is_empty());
    }
}
