//! Ribbon configuration — file paths, git roots, A2A endpoints, agent scopes.
//!
//! Configuration is read from (in order of precedence):
//! 1. Command-line flags (--config, --log, --project-root)
//! 2. Environment variables (RIBBON_CONFIG, RIBBON_LOG_PATH, RIBBON_PROJECT_ROOT)
//! 3. `.ribbon/config.toml` — searched upward from cwd (or --project-root)
//! 4. Built-in defaults
//!
//! Scope configuration is loaded from `.ribbon/scope.toml` alongside config.toml.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level ribbon configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RibbonConfig {
    /// Path to the ndjson event log (relative to config file or absolute).
    #[serde(default = "default_log_path")]
    pub log_path: PathBuf,

    /// Known agents (for validation and status).
    #[serde(default)]
    pub agents: Vec<String>,

    /// Git roots for commit verification.
    /// Maps agent name → path to git repository.
    #[serde(default)]
    pub git_roots: HashMap<String, PathBuf>,

    /// Optional A2A endpoint for bridging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a2a_url: Option<String>,

    /// Remote name for git verification.
    #[serde(default = "default_remote")]
    pub git_remote: String,

    /// Branch name for git verification.
    #[serde(default = "default_branch")]
    pub git_branch: String,

    /// Timezone for rendering (default: UTC).
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

fn default_log_path() -> PathBuf {
    PathBuf::from("events.ndjson")
}

fn default_remote() -> String {
    "origin".to_string()
}

fn default_branch() -> String {
    "main".to_string()
}

fn default_timezone() -> String {
    "UTC".to_string()
}

impl Default for RibbonConfig {
    fn default() -> Self {
        RibbonConfig {
            log_path: default_log_path(),
            agents: Vec::new(),
            git_roots: HashMap::new(),
            a2a_url: None,
            git_remote: default_remote(),
            git_branch: default_branch(),
            timezone: default_timezone(),
        }
    }
}

/// Result of config discovery — includes the config file's directory
/// for resolving relative paths (like log_path) correctly.
#[derive(Debug, Clone)]
pub struct DiscoveredConfig {
    pub config: RibbonConfig,
    /// Directory containing the config file. Used to resolve relative log_path.
    pub config_dir: Option<PathBuf>,
}

impl DiscoveredConfig {
    /// Resolve the log path relative to the config file's directory.
    /// If config_dir is None (no config file found, using defaults),
    /// falls back to the current directory.
    pub fn resolve_log_path(&self) -> PathBuf {
        if self.config.log_path.is_absolute() {
            return self.config.log_path.clone();
        }

        match &self.config_dir {
            Some(dir) => dir.join(&self.config.log_path),
            None => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                cwd.join(&self.config.log_path)
            }
        }
    }
}

impl RibbonConfig {
    /// Search for and load configuration from `.ribbon/config.toml`.
    ///
    /// Searches upward from `search_root` (or cwd if None) to the filesystem root.
    /// Returns the config AND the directory containing the config file.
    pub fn discover_from(search_root: Option<&Path>) -> Result<DiscoveredConfig, ConfigError> {
        let config_path = find_config_file_from(search_root)?;

        match config_path {
            Some(ref path) => {
                let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
                    path: path.clone(),
                    source: e,
                })?;
                let config: RibbonConfig =
                    toml::from_str(&content).map_err(|e| ConfigError::Parse {
                        path: path.clone(),
                        source: e,
                    })?;
                // config_dir = project root (parent of .ribbon/)
                let config_dir = path
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf());
                Ok(DiscoveredConfig { config, config_dir })
            }
            None => Ok(DiscoveredConfig {
                config: RibbonConfig::default(),
                config_dir: None,
            }),
        }
    }

    /// Search upward from the current directory.
    pub fn discover() -> Result<DiscoveredConfig, ConfigError> {
        Self::discover_from(None)
    }

    /// Load config from a specific path. Returns config_dir = parent of path.
    pub fn from_file(path: &Path) -> Result<DiscoveredConfig, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let config: RibbonConfig = toml::from_str(&content).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            source: e,
        })?;
        // config_dir = project root (parent of .ribbon/)
        let config_dir = path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());
        Ok(DiscoveredConfig { config, config_dir })
    }

    /// Override with a specific log path.
    pub fn with_log_path(mut self, path: PathBuf) -> Self {
        self.log_path = path;
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("TOML parse error in {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

/// Search for `.ribbon/config.toml` starting from `search_root` (or cwd) upward.
fn find_config_file_from(search_root: Option<&Path>) -> Result<Option<PathBuf>, ConfigError> {
    let start = match search_root {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            cwd.join(p)
        }
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let mut current = Some(start.as_path());

    while let Some(dir) = current {
        let candidate = dir.join(".ribbon").join("config.toml");
        if candidate.exists() {
            return Ok(Some(candidate));
        }
        current = dir.parent();
    }

    Ok(None)
}

// ── Scope configuration ───────────────────────────────────────────────

/// Top-level scope configuration loaded from `.ribbon/scope.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScopeConfig {
    /// Per-agent scope definitions.
    #[serde(default)]
    pub agents: HashMap<String, ScopeEntry>,
}

/// What a single agent owns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeEntry {
    /// Glob patterns for owned paths (e.g. "submodules/mosaic/**").
    #[serde(default)]
    pub paths: Vec<String>,

    /// Docker Compose service names this agent manages.
    #[serde(default)]
    pub docker_services: Vec<String>,
}

/// Result of matching a directory against all agent scopes.
#[derive(Debug, Clone)]
pub struct WhoamiResult {
    /// The matched agent name.
    pub agent: String,
    /// Owned path patterns.
    pub paths: Vec<String>,
    /// Docker services.
    pub docker_services: Vec<String>,
    /// Git root path (from config.toml), if configured.
    pub git_root: Option<PathBuf>,
    /// Project root directory (where .ribbon/ lives).
    pub project_root: PathBuf,
    /// Relative paths to peer agents' scopes (for context).
    pub peers: Vec<(String, Vec<String>)>,
}

impl ScopeConfig {
    /// Load scope.toml from a specific path.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        toml::from_str(&content).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            source: e,
        })
    }

    /// Discover scope.toml by finding the project root (where .ribbon/ lives)
    /// and loading scope.toml from there.
    pub fn discover_from(
        search_root: Option<&Path>,
    ) -> Result<Option<(Self, PathBuf)>, ConfigError> {
        let ribbon_dir = find_ribbon_dir_from(search_root)?;
        match ribbon_dir {
            Some(ribbon_dir) => {
                let scope_path = ribbon_dir.join("scope.toml");
                if scope_path.exists() {
                    let config = Self::from_file(&scope_path)?;
                    let project_root = ribbon_dir
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or(ribbon_dir);
                    Ok(Some((config, project_root)))
                } else {
                    // scope.toml is optional — no scope info available
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Determine which agent owns the given working directory.
    /// Matches `cwd` against each agent's path globs.
    /// Returns None if no scope matches (agent is outside all scopes).
    pub fn whoami(
        &self,
        cwd: &Path,
        project_root: &Path,
        config: &RibbonConfig,
    ) -> Option<WhoamiResult> {
        let cwd_abs = if cwd.is_absolute() {
            cwd.to_path_buf()
        } else {
            std::env::current_dir().ok()?.join(cwd)
        };
        let project_root_abs = if project_root.is_absolute() {
            project_root.to_path_buf()
        } else {
            std::env::current_dir().ok()?.join(project_root)
        };

        // Try to relativize cwd against project root for glob matching
        let cwd_rel = cwd_abs.strip_prefix(&project_root_abs).ok()?;
        let cwd_str = cwd_rel.to_string_lossy();

        for (agent, entry) in &self.agents {
            for pattern in &entry.paths {
                // Strip trailing /** for directory-level matching
                let dir_pattern = pattern.trim_end_matches("/**");
                // Also support exact match with trailing /**
                let glob_pattern = if pattern.ends_with("/**") {
                    format!("{}/**", dir_pattern)
                } else {
                    pattern.clone()
                };

                // Check if cwd starts with the directory pattern
                if cwd_str == dir_pattern || cwd_str.starts_with(&format!("{dir_pattern}/")) {
                    // Also validate with glob for ** patterns
                    if pattern.ends_with("/**") {
                        if let Ok(matched) = glob::Pattern::new(&glob_pattern) {
                            if matched.matches(&cwd_str) || matched.matches(&format!("{cwd_str}/"))
                            {
                                // Build peers list (all other agents and their paths)
                                let peers: Vec<(String, Vec<String>)> = self
                                    .agents
                                    .iter()
                                    .filter(|(a, _)| *a != agent)
                                    .map(|(a, e)| (a.clone(), e.paths.clone()))
                                    .collect();

                                let git_root = config.git_roots.get(agent).cloned();

                                return Some(WhoamiResult {
                                    agent: agent.clone(),
                                    paths: entry.paths.clone(),
                                    docker_services: entry.docker_services.clone(),
                                    git_root,
                                    project_root: project_root_abs,
                                    peers,
                                });
                            }
                        }
                    } else {
                        // Non-glob pattern: check prefix match with glob
                        let simple_pattern = format!("{dir_pattern}/**");
                        if let Ok(matched) = glob::Pattern::new(&simple_pattern) {
                            if matched.matches(&cwd_str) || matched.matches(&format!("{cwd_str}/"))
                            {
                                let peers: Vec<(String, Vec<String>)> = self
                                    .agents
                                    .iter()
                                    .filter(|(a, _)| *a != agent)
                                    .map(|(a, e)| (a.clone(), e.paths.clone()))
                                    .collect();

                                let git_root = config.git_roots.get(agent).cloned();

                                return Some(WhoamiResult {
                                    agent: agent.clone(),
                                    paths: entry.paths.clone(),
                                    docker_services: entry.docker_services.clone(),
                                    git_root,
                                    project_root: project_root_abs,
                                    peers,
                                });
                            }
                        }
                    }
                }
            }
        }

        None
    }
}

/// Search for `.ribbon/` directory starting from `search_root` (or cwd) upward.
fn find_ribbon_dir_from(search_root: Option<&Path>) -> Result<Option<PathBuf>, ConfigError> {
    let start = match search_root {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            cwd.join(p)
        }
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let mut current = Some(start.as_path());

    while let Some(dir) = current {
        let candidate = dir.join(".ribbon");
        if candidate.is_dir() {
            return Ok(Some(candidate));
        }
        current = dir.parent();
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RibbonConfig::default();
        assert_eq!(config.log_path, PathBuf::from("events.ndjson"));
        assert_eq!(config.git_remote, "origin");
        assert_eq!(config.git_branch, "main");
    }

    #[test]
    fn test_discovered_config_resolve_log_path() {
        let config = RibbonConfig::default();
        let dc = DiscoveredConfig {
            config,
            config_dir: Some(PathBuf::from("/home/user/project")),
        };
        // log_path is relative, should resolve against project root
        let resolved = dc.resolve_log_path();
        assert_eq!(resolved, PathBuf::from("/home/user/project/events.ndjson"));
    }

    // ── ScopeConfig / whoami tests ────────────────────────────────────

    fn make_scope_config() -> ScopeConfig {
        let mut agents = HashMap::new();
        agents.insert(
            "mosaic".to_string(),
            ScopeEntry {
                paths: vec!["submodules/mosaic/**".to_string()],
                docker_services: vec!["mosaic".to_string()],
            },
        );
        agents.insert(
            "zypi".to_string(),
            ScopeEntry {
                paths: vec!["submodules/zypi/**".to_string()],
                docker_services: vec!["zypi".to_string()],
            },
        );
        agents.insert(
            "weft".to_string(),
            ScopeEntry {
                paths: vec![
                    "weft-core/**".to_string(),
                    "shared/**".to_string(),
                    "Cargo.toml".to_string(),
                ],
                docker_services: vec!["weft".to_string()],
            },
        );
        ScopeConfig { agents }
    }

    fn make_ribbon_config() -> RibbonConfig {
        let mut git_roots = HashMap::new();
        git_roots.insert("mosaic".to_string(), PathBuf::from("submodules/mosaic"));
        git_roots.insert("zypi".to_string(), PathBuf::from("submodules/zypi"));
        RibbonConfig {
            agents: vec!["mosaic".to_string(), "zypi".to_string(), "weft".to_string()],
            git_roots,
            ..RibbonConfig::default()
        }
    }

    #[test]
    fn test_whoami_matches_top_level_scope() {
        let scope = make_scope_config();
        let config = make_ribbon_config();
        let project_root = PathBuf::from("/tmp/test-project");

        // cwd is inside mosaic's scope
        let cwd = PathBuf::from("/tmp/test-project/submodules/mosaic");
        let result = scope.whoami(&cwd, &project_root, &config);
        assert!(result.is_some(), "should match mosaic scope");
        let r = result.unwrap();
        assert_eq!(r.agent, "mosaic");
        assert_eq!(r.paths, vec!["submodules/mosaic/**"]);
        assert_eq!(r.docker_services, vec!["mosaic"]);
        assert_eq!(r.git_root, Some(PathBuf::from("submodules/mosaic")));
        assert_eq!(r.project_root, PathBuf::from("/tmp/test-project"));
        // Peers should include zypi and weft but NOT mosaic
        assert_eq!(r.peers.len(), 2);
        let peer_names: Vec<&str> = r.peers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(peer_names.contains(&"zypi"));
        assert!(peer_names.contains(&"weft"));
        assert!(!peer_names.contains(&"mosaic"));
    }

    #[test]
    fn test_whoami_matches_subdirectory() {
        let scope = make_scope_config();
        let config = make_ribbon_config();
        let project_root = PathBuf::from("/tmp/test-project");

        // cwd inside a subdirectory of mosaic
        let cwd = PathBuf::from("/tmp/test-project/submodules/mosaic/lib");
        let result = scope.whoami(&cwd, &project_root, &config);
        assert!(result.is_some(), "subdirectory should match mosaic/**");
        assert_eq!(result.unwrap().agent, "mosaic");
    }

    #[test]
    fn test_whoami_matches_deep_subdirectory() {
        let scope = make_scope_config();
        let config = make_ribbon_config();
        let project_root = PathBuf::from("/tmp/test-project");

        // Deep nesting
        let cwd = PathBuf::from("/tmp/test-project/submodules/mosaic/priv/some/deep/path");
        let result = scope.whoami(&cwd, &project_root, &config);
        assert!(result.is_some(), "deep paths should match /**");
        assert_eq!(result.unwrap().agent, "mosaic");
    }

    #[test]
    fn test_whoami_matches_weft_scope_multiple_paths() {
        let scope = make_scope_config();
        let config = make_ribbon_config();
        let project_root = PathBuf::from("/tmp/test-project");

        // weft owns weft-core/**
        let cwd = PathBuf::from("/tmp/test-project/weft-core");
        let result = scope.whoami(&cwd, &project_root, &config);
        assert!(result.is_some(), "should match weft via weft-core/**");
        assert_eq!(result.unwrap().agent, "weft");
    }

    #[test]
    fn test_whoami_no_match_outside_scope() {
        let scope = make_scope_config();
        let config = make_ribbon_config();
        let project_root = PathBuf::from("/tmp/test-project");

        // Project root itself doesn't match any scope (weft owns weft-core/**, not .)
        let cwd = PathBuf::from("/tmp/test-project");
        let result = scope.whoami(&cwd, &project_root, &config);
        assert!(result.is_none(), "project root should not match any scope");
    }

    #[test]
    fn test_whoami_no_match_random_dir() {
        let scope = make_scope_config();
        let config = make_ribbon_config();
        let project_root = PathBuf::from("/tmp/test-project");

        // Random directory
        let cwd = PathBuf::from("/tmp/test-project/some-random-folder");
        let result = scope.whoami(&cwd, &project_root, &config);
        assert!(result.is_none(), "random dir should not match");
    }

    #[test]
    fn test_scope_config_from_valid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scope.toml");
        std::fs::write(
            &path,
            r#"
[agents.test-agent]
paths = ["src/**", "tests/**"]
docker_services = ["test-svc"]
"#,
        )
        .unwrap();

        let config = ScopeConfig::from_file(&path).unwrap();
        assert!(config.agents.contains_key("test-agent"));
        let entry = config.agents.get("test-agent").unwrap();
        assert_eq!(entry.paths, vec!["src/**", "tests/**"]);
        assert_eq!(entry.docker_services, vec!["test-svc"]);
    }

    #[test]
    fn test_whoami_empty_scope_config() {
        let scope = ScopeConfig::default();
        let config = RibbonConfig::default();
        let project_root = PathBuf::from("/tmp/test-project");
        let cwd = PathBuf::from("/tmp/test-project/submodules/mosaic");
        let result = scope.whoami(&cwd, &project_root, &config);
        assert!(result.is_none(), "empty scope should match nothing");
    }

    #[test]
    fn test_whoami_exact_file_match() {
        let mut agents = HashMap::new();
        agents.insert(
            "weft".to_string(),
            ScopeEntry {
                paths: vec!["Cargo.toml".to_string()],
                docker_services: vec![],
            },
        );
        let scope = ScopeConfig { agents };
        let config = RibbonConfig::default();
        let project_root = PathBuf::from("/tmp/test-project");

        // cwd is project root — Cargo.toml is a file pattern, not a directory
        let cwd = PathBuf::from("/tmp/test-project");
        let result = scope.whoami(&cwd, &project_root, &config);
        // File patterns like "Cargo.toml" won't match against cwd (directories)
        // This is expected — whoami matches directories, not files
        assert!(result.is_none(), "file patterns should not match cwd");
    }

    #[test]
    fn test_whoami_matches_weft_shared_scope() {
        let scope = make_scope_config();
        let config = make_ribbon_config();
        let project_root = PathBuf::from("/tmp/test-project");

        // weft also owns shared/**
        let cwd = PathBuf::from("/tmp/test-project/shared");
        let result = scope.whoami(&cwd, &project_root, &config);
        assert!(result.is_some(), "should match weft via shared/**");
        assert_eq!(result.unwrap().agent, "weft");
    }
}
