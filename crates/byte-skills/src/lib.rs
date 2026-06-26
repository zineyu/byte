use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use byte_protocol::{SkillDefinition, SkillEntry};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// An error produced while scanning or activating skills.
#[derive(Debug, Error)]
pub enum SkillError {
    #[error("skill not found: {0}")]
    NotFound(String),
    #[error("failed to read skill directory: {0}")]
    ReadDir(#[source] std::io::Error),
    #[error("failed to read skill file {0}: {1}")]
    ReadFile(PathBuf, #[source] std::io::Error),
    #[error("skill file missing required `name` frontmatter: {0}")]
    MissingName(PathBuf),
    #[error("invalid frontmatter in skill file {0}: {1}")]
    InvalidFrontmatter(PathBuf, String),
}

/// A registry that discovers and activates agent skills.
#[async_trait]
pub trait SkillRegistry: Send + Sync {
    /// Return the list of available skills for the given workspace.
    async fn catalog(&self, workspace: Option<&Path>) -> Result<Vec<SkillEntry>, SkillError>;

    /// Activate a skill by name, returning its full definition.
    async fn activate(
        &self,
        workspace: Option<&Path>,
        name: &str,
    ) -> Result<SkillDefinition, SkillError>;
}

/// Cache of scanned skills keyed by workspace root.
type SkillScanCache = HashMap<Option<PathBuf>, HashMap<String, SkillDefinition>>;

/// An in-memory skill registry used in the MVP.
#[derive(Debug, Clone)]
pub struct MvpSkillRegistry {
    home_dir: Option<PathBuf>,
    /// Cache of scanned skills keyed by workspace root. Scanning is done once
    /// per workspace and reused across `catalog` and `activate` calls to avoid
    /// repeated disk reads.
    scan_cache: Arc<Mutex<SkillScanCache>>,
}

impl MvpSkillRegistry {
    /// Create a new skill registry that discovers user skills under `$HOME`.
    pub fn new() -> Self {
        Self {
            home_dir: home_dir(),
            scan_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a skill registry with an explicit home directory for testing.
    pub fn with_home_dir(home_dir: impl Into<PathBuf>) -> Self {
        Self {
            home_dir: Some(home_dir.into()),
            scan_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn skill_dirs(&self, workspace: Option<&Path>) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        // User-level skills are scanned first so that workspace-specific skills
        // (scanned later) override them, matching the acceptance criteria that
        // project-specific rules take precedence.
        if let Some(home) = &self.home_dir {
            dirs.push(home.join(".agents").join("skills"));
            dirs.push(home.join(".byte").join("skills"));
        }
        if let Some(ws) = workspace {
            dirs.push(ws.join(".agents").join("skills"));
            dirs.push(ws.join(".byte").join("skills"));
        }
        dirs
    }
}

impl Default for MvpSkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SkillRegistry for MvpSkillRegistry {
    async fn catalog(&self, workspace: Option<&Path>) -> Result<Vec<SkillEntry>, SkillError> {
        let skills = self.scan(workspace).await?;
        let mut entries: Vec<SkillEntry> = skills
            .into_values()
            .map(|definition| SkillEntry {
                name: definition.name,
                description: definition.description,
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    async fn activate(
        &self,
        workspace: Option<&Path>,
        name: &str,
    ) -> Result<SkillDefinition, SkillError> {
        let skills = self.scan(workspace).await?;
        skills
            .into_values()
            .find(|definition| definition.name == name)
            .ok_or_else(|| SkillError::NotFound(name.to_owned()))
    }
}

impl MvpSkillRegistry {
    /// Return cached skills for `workspace` or scan the skill directories once
    /// and store the result.
    async fn scan(
        &self,
        workspace: Option<&Path>,
    ) -> Result<HashMap<String, SkillDefinition>, SkillError> {
        let key = workspace.map(Path::to_path_buf);
        {
            let cache = self.scan_cache.lock().await;
            if let Some(skills) = cache.get(&key) {
                return Ok(skills.clone());
            }
        }

        let skills = scan_skills(self.skill_dirs(workspace)).await?;
        let mut cache = self.scan_cache.lock().await;
        cache.insert(key, skills.clone());
        Ok(skills)
    }
}

async fn scan_skills(dirs: Vec<PathBuf>) -> Result<HashMap<String, SkillDefinition>, SkillError> {
    let mut skills: HashMap<String, SkillDefinition> = HashMap::new();

    for dir in dirs {
        match tokio::fs::metadata(&dir).await {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                debug!(dir = %dir.display(), "skipping skill path that is not a directory");
                continue;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                debug!(dir = %dir.display(), "skill directory does not exist");
                continue;
            }
            Err(error) => {
                warn!(dir = %dir.display(), %error, "failed to read skill directory metadata");
                continue;
            }
        }
        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .map_err(SkillError::ReadDir)?;
        while let Some(entry) = entries.next_entry().await.map_err(SkillError::ReadDir)? {
            let path = entry.path();
            match tokio::fs::metadata(&path).await {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => continue,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    debug!(path = %path.display(), "skill entry does not exist");
                    continue;
                }
                Err(error) => {
                    warn!(path = %path.display(), %error, "failed to read skill entry metadata");
                    continue;
                }
            }
            let skill_md = path.join("skill.md");
            match tokio::fs::metadata(&skill_md).await {
                Ok(meta) if meta.is_file() => {}
                Ok(_) => continue,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    debug!(path = %skill_md.display(), "skill.md does not exist");
                    continue;
                }
                Err(error) => {
                    warn!(path = %skill_md.display(), %error, "failed to read skill.md metadata");
                    continue;
                }
            }
            match parse_skill_file(&skill_md).await {
                Ok(definition) => {
                    debug!(
                        name = %definition.name,
                        path = %skill_md.display(),
                        "discovered skill"
                    );
                    skills.insert(definition.name.clone(), definition);
                }
                Err(error) => {
                    warn!(path = %skill_md.display(), %error, "failed to parse skill file");
                    // Continue scanning other skills; a malformed file should not
                    // break the whole catalog.
                }
            }
        }
    }

    Ok(skills)
}

async fn parse_skill_file(path: &Path) -> Result<SkillDefinition, SkillError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| SkillError::ReadFile(path.to_path_buf(), error))?;

    let (frontmatter, body) = split_frontmatter(&content);
    let name = frontmatter
        .get("name")
        .cloned()
        .ok_or_else(|| SkillError::MissingName(path.to_path_buf()))?;
    let description = frontmatter
        .get("description")
        .cloned()
        .unwrap_or_else(|| first_markdown_heading(body).unwrap_or_default());

    Ok(SkillDefinition {
        name,
        description,
        content: body.trim().to_owned(),
    })
}

fn split_frontmatter(content: &str) -> (HashMap<String, String>, &str) {
    let trimmed = content.trim_start();
    let Some(first_line) = trimmed.lines().next() else {
        return (HashMap::new(), content);
    };
    // The opening delimiter must be on its own line, not a substring of a
    // larger line such as `---foo`.
    if first_line.trim_end() != "---" {
        return (HashMap::new(), content);
    }

    let after_open = &trimmed[first_line.len().saturating_add(1)..];
    let mut offset = 0;
    for line in after_open.lines() {
        let trimmed_line = line.trim_start();
        if let Some(rest) = trimmed_line.strip_prefix("---")
            && rest.trim().is_empty()
        {
            let close_start = offset + (line.len() - trimmed_line.len());
            let frontmatter_text = &after_open[..close_start];
            let body = &after_open[close_start + 3..];
            let frontmatter = parse_simple_frontmatter(frontmatter_text);
            return (frontmatter, body);
        }
        offset += line.len().saturating_add(1);
    }

    (HashMap::new(), content)
}

fn parse_simple_frontmatter(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_owned();
            let value = value.trim().to_owned();
            if !key.is_empty() {
                map.insert(key, value);
            }
        }
    }
    map
}

fn first_markdown_heading(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let heading = rest.trim().trim_start_matches('#').trim();
            if !heading.is_empty() {
                return Some(heading.to_owned());
            }
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frontmatter_value_preserves_colons() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let skill_dir = workspace.join(".byte").join("skills").join("review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("skill.md"),
            "---\nname: review\ndescription: Review: do this\n---\n# Code Review\n",
        )
        .unwrap();

        let registry = MvpSkillRegistry::with_home_dir(temp.path());
        let catalog = registry.catalog(Some(&workspace)).await.unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].name, "review");
        assert_eq!(catalog[0].description, "Review: do this");
    }

    #[test]
    fn frontmatter_ignores_substring_delimiter_inside_value() {
        let content = "---\nname: review\ndescription: foo --- bar\n---\n# Body\n";
        let (frontmatter, body) = split_frontmatter(content);
        assert_eq!(
            frontmatter.get("description"),
            Some(&"foo --- bar".to_owned())
        );
        assert_eq!(body.trim(), "# Body");
    }

    #[test]
    fn frontmatter_requires_opening_delimiter_on_own_line() {
        // `---x` on the first line must not be treated as a frontmatter opener.
        let content = "---x\nname: review\n---\n# Body\n";
        let (frontmatter, body) = split_frontmatter(content);
        assert!(frontmatter.is_empty());
        assert_eq!(body, content);
    }

    #[tokio::test]
    async fn catalog_is_sorted_by_name() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let skills_dir = workspace.join(".byte").join("skills");

        let zebra_dir = skills_dir.join("zebra");
        let alpha_dir = skills_dir.join("alpha");
        std::fs::create_dir_all(&zebra_dir).unwrap();
        std::fs::create_dir_all(&alpha_dir).unwrap();
        std::fs::write(
            zebra_dir.join("skill.md"),
            "---\nname: zebra\ndescription: last\n---\n",
        )
        .unwrap();
        std::fs::write(
            alpha_dir.join("skill.md"),
            "---\nname: alpha\ndescription: first\n---\n",
        )
        .unwrap();

        let registry = MvpSkillRegistry::with_home_dir(temp.path());
        let catalog = registry.catalog(Some(&workspace)).await.unwrap();
        let names: Vec<_> = catalog.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zebra"]);
    }

    #[tokio::test]
    async fn scan_prefers_workspace_skill_over_user_skill() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let workspace_skill = workspace.join(".byte").join("skills").join("review");
        let user_skill = home.join(".byte").join("skills").join("review");
        std::fs::create_dir_all(&workspace_skill).unwrap();
        std::fs::create_dir_all(&user_skill).unwrap();

        std::fs::write(
            workspace_skill.join("skill.md"),
            "---\nname: review\ndescription: workspace review\n---\n# Workspace Review\n\nReview workspace code.",
        )
        .unwrap();
        std::fs::write(
            user_skill.join("skill.md"),
            "---\nname: review\ndescription: user review\n---\n# User Review\n\nReview user code.",
        )
        .unwrap();

        let registry = MvpSkillRegistry::with_home_dir(&home);

        let catalog = registry.catalog(Some(&workspace)).await.unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].name, "review");
        assert_eq!(catalog[0].description, "workspace review");

        let definition = registry.activate(Some(&workspace), "review").await.unwrap();
        assert_eq!(definition.name, "review");
        assert!(definition.content.contains("Review workspace code."));

        let catalog_no_workspace = registry.catalog(None).await.unwrap();
        assert_eq!(catalog_no_workspace.len(), 1);
        assert_eq!(catalog_no_workspace[0].description, "user review");
    }

    #[tokio::test]
    async fn description_falls_back_to_first_heading() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let skill_dir = workspace.join(".byte").join("skills").join("review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("skill.md"),
            "---\nname: review\n---\n# Code Review Skill\n\nAlways review carefully.",
        )
        .unwrap();

        let registry = MvpSkillRegistry::with_home_dir(temp.path());
        let catalog = registry.catalog(Some(&workspace)).await.unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].name, "review");
        assert_eq!(catalog[0].description, "Code Review Skill");
    }

    #[tokio::test]
    async fn scan_order_allows_later_directory_to_override() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let agents_dir = workspace.join(".agents").join("skills").join("review");
        let byte_dir = workspace.join(".byte").join("skills").join("review");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::create_dir_all(&byte_dir).unwrap();

        std::fs::write(
            agents_dir.join("skill.md"),
            "---\nname: review\ndescription: agents\n---\ncontent",
        )
        .unwrap();
        std::fs::write(
            byte_dir.join("skill.md"),
            "---\nname: review\ndescription: byte\n---\ncontent",
        )
        .unwrap();

        let registry = MvpSkillRegistry::with_home_dir(temp.path());
        let catalog = registry.catalog(Some(&workspace)).await.unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].description, "byte");
    }

    #[tokio::test]
    async fn activate_uses_cached_scan_results() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let skill_dir = workspace.join(".byte").join("skills").join("review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("skill.md"),
            "---\nname: review\ndescription: cached\n---\nCached content.",
        )
        .unwrap();

        let registry = MvpSkillRegistry::with_home_dir(temp.path());
        let first = registry.activate(Some(&workspace), "review").await.unwrap();
        assert_eq!(first.content, "Cached content.");

        // Remove the skill directory to prove the second call uses the cache
        // instead of rescanning disk.
        std::fs::remove_dir_all(&skill_dir).unwrap();

        let second = registry.activate(Some(&workspace), "review").await.unwrap();
        assert_eq!(second.content, "Cached content.");
    }

    #[tokio::test]
    async fn activate_returns_not_found_for_missing_skill() {
        let temp = tempfile::tempdir().unwrap();
        let registry = MvpSkillRegistry::with_home_dir(temp.path());
        let result = registry.activate(Some(temp.path()), "missing").await;
        assert!(matches!(result, Err(SkillError::NotFound(name)) if name == "missing"));
    }

    #[tokio::test]
    async fn explicit_home_dir_is_scanned() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let skill_dir = home.join(".byte").join("skills").join("review");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("skill.md"),
            "---\nname: review\ndescription: from home\n---\nHome skill.",
        )
        .unwrap();

        let registry = MvpSkillRegistry::with_home_dir(&home);
        let catalog = registry.catalog(None).await.unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].name, "review");
        assert_eq!(catalog[0].description, "from home");
    }

    #[test]
    fn first_markdown_heading_extracts_text() {
        assert_eq!(
            first_markdown_heading("# Hello\n\nWorld"),
            Some("Hello".to_owned())
        );
        assert_eq!(first_markdown_heading("## Hello"), Some("Hello".to_owned()));
        assert_eq!(first_markdown_heading("no heading"), None);
    }
}
