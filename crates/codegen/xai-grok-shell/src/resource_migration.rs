//! Source-preserving migration of portable skills and agent definitions.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use xai_grok_agent::config::AgentDefinition;
use xai_grok_tools::implementations::skills::discovery::{
    is_valid_skill_name, parse_skill_frontmatter,
};

const MANIFEST_NAME: &str = "migration-provenance.json";
const SOURCE_ROOTS: &[&str] = &[".claude", ".codex", ".agents", ".grok"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MigrationScope {
    Project,
    Global,
}

#[derive(Debug, Clone)]
pub struct PluginResources {
    pub name: String,
    pub version: Option<String>,
    pub scope: MigrationScope,
    pub root: PathBuf,
    pub skill_dirs: Vec<PathBuf>,
    pub agent_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct MigrationOptions {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub dry_run: bool,
    pub plugin_resources: Vec<PluginResources>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub copied: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub invalid: Vec<String>,
    pub collisions: Vec<String>,
    pub dry_run: bool,
}

impl MigrationReport {
    pub fn summary(&self) -> String {
        format!(
            "{}{} copied, {} updated, {} unchanged, {} invalid",
            if self.dry_run { "dry-run: " } else { "" },
            self.copied,
            self.updated,
            self.unchanged,
            self.invalid.len()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ResourceKind {
    Skill,
    Agent,
}

#[derive(Debug, Clone)]
struct Candidate {
    kind: ResourceKind,
    name: String,
    source: PathBuf,
    source_key: String,
    target_root: PathBuf,
    target: PathBuf,
    target_key: String,
    scope: MigrationScope,
    hash: String,
    plugin: Option<String>,
    plugin_version: Option<String>,
    adapted_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProvenanceManifest {
    schema_version: u32,
    entries: Vec<ProvenanceEntry>,
}

impl Default for ProvenanceManifest {
    fn default() -> Self {
        Self {
            schema_version: 1,
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProvenanceEntry {
    kind: ResourceKind,
    name: String,
    scope: MigrationScope,
    source: String,
    target: String,
    content_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Copy,
    Update,
    Unchanged,
}

/// Migrate project/global resources into canonical `.bandicot` roots.
///
/// Sources are never modified. Every destination is preflighted before any
/// writes occur, and an existing destination is replaceable only when the
/// provenance manifest proves it was created from the same source.
pub fn migrate_resources(options: &MigrationOptions) -> Result<MigrationReport> {
    let project_root = crate::claude_import::find_project_root(&options.cwd);
    let project_target = project_root.join(".bandicot");
    let global_target = options.home.join(".bandicot");
    let mut report = MigrationReport {
        dry_run: options.dry_run,
        ..MigrationReport::default()
    };
    let mut candidates = Vec::new();

    if canonical_or_original(&project_root) != canonical_or_original(&options.home) {
        collect_owned_roots(
            &project_root,
            &project_target,
            MigrationScope::Project,
            &mut candidates,
            &mut report.invalid,
        );
    }
    collect_owned_roots(
        &options.home,
        &global_target,
        MigrationScope::Global,
        &mut candidates,
        &mut report.invalid,
    );
    for plugin in &options.plugin_resources {
        let target = match plugin.scope {
            MigrationScope::Project => &project_target,
            MigrationScope::Global => &global_target,
        };
        collect_plugin_resources(plugin, target, &mut candidates, &mut report.invalid);
    }

    qualify_competing_skills(&mut candidates);
    candidates.sort_by(|a, b| (&a.target_key, &a.source_key).cmp(&(&b.target_key, &b.source_key)));
    candidates.dedup_by(|a, b| a.source_key == b.source_key && a.target_key == b.target_key);

    let manifests = load_manifests(&project_target, &global_target)?;
    let actions = preflight(&candidates, &manifests, &mut report)?;
    if !report.collisions.is_empty() {
        bail!(
            "resource migration refused due to collisions:\n{}",
            report.collisions.join("\n")
        );
    }

    for action in &actions {
        match action {
            Action::Copy => report.copied += 1,
            Action::Update => report.updated += 1,
            Action::Unchanged => report.unchanged += 1,
        }
    }
    if options.dry_run {
        return Ok(report);
    }

    let mut updated_manifests = manifests;
    for (candidate, action) in candidates.iter().zip(actions) {
        if action == Action::Unchanged {
            continue;
        }
        promote_candidate(candidate, action == Action::Update)?;
        upsert_manifest_entry(
            updated_manifests
                .get_mut(&candidate.target_root)
                .expect("manifest initialized for each target root"),
            candidate,
        );
    }
    if report.copied > 0 || report.updated > 0 {
        write_manifests(&updated_manifests)?;
    }
    Ok(report)
}

fn qualify_competing_skills(candidates: &mut Vec<Candidate>) {
    candidates.sort_by(|a, b| {
        let preference = |candidate: &Candidate| {
            let basename_matches = candidate
                .source
                .file_name()
                .and_then(|value| value.to_str())
                == Some(candidate.name.as_str());
            (
                candidate.plugin.is_some(),
                candidate.source_key.contains("/.system/"),
                !basename_matches,
                candidate.source_key.len(),
            )
        };
        (&a.target_key, preference(a))
            .cmp(&(&b.target_key, preference(b)))
            .then_with(|| a.source_key.cmp(&b.source_key))
    });

    let mut occupied: HashMap<(PathBuf, String), String> = HashMap::new();
    let mut resolved = Vec::with_capacity(candidates.len());
    for mut candidate in candidates.drain(..) {
        let key = (candidate.target_root.clone(), candidate.target_key.clone());
        if let Some(existing_hash) = occupied.get(&key) {
            if existing_hash == &candidate.hash {
                continue;
            }
            if candidate.kind != ResourceKind::Skill {
                resolved.push(candidate);
                continue;
            }
            let source_basename = candidate
                .source
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| is_valid_skill_name(value));
            let prefix = candidate
                .plugin
                .as_deref()
                .map(portable_slug)
                .filter(|value| !value.is_empty());
            let base = source_basename
                .filter(|value| *value != candidate.name)
                .map(str::to_owned)
                .or_else(|| prefix.map(|value| format!("{value}-{}", candidate.name)))
                .unwrap_or_else(|| format!("imported-{}", candidate.name));
            let mut qualified = base.clone();
            let mut suffix = 1_u32;
            loop {
                candidate.target = candidate.target_root.join("skills").join(&qualified);
                candidate.target_key = format!("skills/{qualified}");
                let qualified_key = (candidate.target_root.clone(), candidate.target_key.clone());
                if !occupied.contains_key(&qualified_key) {
                    candidate.name = qualified;
                    occupied.insert(qualified_key, candidate.hash.clone());
                    break;
                }
                suffix += 1;
                qualified = format!("{base}-{suffix}");
            }
        } else {
            occupied.insert(key, candidate.hash.clone());
        }
        resolved.push(candidate);
    }
    *candidates = resolved;
}

fn portable_slug(value: &str) -> String {
    let mut slug = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

fn collect_owned_roots(
    owner: &Path,
    target_root: &Path,
    scope: MigrationScope,
    candidates: &mut Vec<Candidate>,
    invalid: &mut Vec<String>,
) {
    for root_name in SOURCE_ROOTS {
        let root = owner.join(root_name);
        collect_skill_dir(
            &root.join("skills"),
            target_root,
            scope,
            None,
            None,
            candidates,
            invalid,
        );
        collect_agent_dir(
            &root.join("agents"),
            target_root,
            scope,
            None,
            None,
            candidates,
            invalid,
        );
    }
}

fn collect_plugin_resources(
    plugin: &PluginResources,
    target_root: &Path,
    candidates: &mut Vec<Candidate>,
    invalid: &mut Vec<String>,
) {
    let plugin_root = canonical_or_original(&plugin.root);
    for dir in &plugin.skill_dirs {
        if canonical_or_original(dir).starts_with(&plugin_root) {
            collect_skill_dir(
                dir,
                target_root,
                plugin.scope,
                Some(plugin.name.clone()),
                plugin.version.clone(),
                candidates,
                invalid,
            );
        }
    }
    for dir in &plugin.agent_dirs {
        if canonical_or_original(dir).starts_with(&plugin_root) {
            collect_agent_dir(
                dir,
                target_root,
                plugin.scope,
                Some(plugin.name.clone()),
                plugin.version.clone(),
                candidates,
                invalid,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_skill_dir(
    dir: &Path,
    target_root: &Path,
    scope: MigrationScope,
    plugin: Option<String>,
    plugin_version: Option<String>,
    candidates: &mut Vec<Candidate>,
    invalid: &mut Vec<String>,
) {
    if !dir.is_dir() {
        return;
    }
    for entry in WalkDir::new(dir).follow_links(false).into_iter().flatten() {
        if entry.file_type().is_symlink()
            || !entry.file_type().is_file()
            || entry.file_name() != "SKILL.md"
        {
            continue;
        }
        let Some(source_dir) = entry.path().parent() else {
            continue;
        };
        let fallback = source_dir.file_name().and_then(|n| n.to_str());
        let parsed = fs::read_to_string(entry.path())
            .ok()
            .and_then(|body| parse_skill_frontmatter(&body, fallback).ok());
        let Some(parsed) = parsed else {
            invalid.push(format!("invalid skill: {}", source_dir.display()));
            continue;
        };
        if !is_valid_skill_name(&parsed.name) {
            invalid.push(format!("invalid skill name at {}", source_dir.display()));
            continue;
        }
        match make_candidate(
            ResourceKind::Skill,
            parsed.name,
            source_dir,
            target_root,
            scope,
            plugin.clone(),
            plugin_version.clone(),
        ) {
            Ok(candidate) => candidates.push(candidate),
            Err(error) => invalid.push(error.to_string()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_agent_dir(
    dir: &Path,
    target_root: &Path,
    scope: MigrationScope,
    plugin: Option<String>,
    plugin_version: Option<String>,
    candidates: &mut Vec<Candidate>,
    invalid: &mut Vec<String>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
            invalid.push(format!("agent symlink is not portable: {}", path.display()));
            continue;
        }
        let (name, adapted_content) = match AgentDefinition::from_file(&path) {
            Ok(definition) => (definition.name, None),
            Err(_) => match adapt_claude_agent(&path) {
                Ok(adapted) => adapted,
                Err(_) => {
                    invalid.push(format!("invalid agent: {}", path.display()));
                    continue;
                }
            },
        };
        if !is_valid_skill_name(&name) {
            invalid.push(format!("invalid agent name at {}", path.display()));
            continue;
        }
        match make_candidate(
            ResourceKind::Agent,
            name,
            &path,
            target_root,
            scope,
            plugin.clone(),
            plugin_version.clone(),
        ) {
            Ok(mut candidate) => {
                if let Some(content) = adapted_content {
                    candidate.hash = hash_file_bytes(content.as_bytes());
                    candidate.adapted_content = Some(content);
                }
                candidates.push(candidate);
            }
            Err(error) => invalid.push(error.to_string()),
        }
    }
}

fn make_candidate(
    kind: ResourceKind,
    name: String,
    source: &Path,
    target_root: &Path,
    scope: MigrationScope,
    plugin: Option<String>,
    plugin_version: Option<String>,
) -> Result<Candidate> {
    let source = canonical_or_original(source);
    let target = match kind {
        ResourceKind::Skill => target_root.join("skills").join(&name),
        ResourceKind::Agent => target_root.join("agents").join(format!("{name}.md")),
    };
    let target_key = target
        .strip_prefix(target_root)
        .expect("target is built under target root")
        .to_string_lossy()
        .replace('\\', "/");
    Ok(Candidate {
        kind,
        name,
        source_key: source.to_string_lossy().to_string(),
        hash: hash_path(&source)?,
        source,
        target_root: target_root.to_path_buf(),
        target,
        target_key,
        scope,
        plugin,
        plugin_version,
        adapted_content: None,
    })
}

fn adapt_claude_agent(path: &Path) -> Result<(String, Option<String>)> {
    let content = fs::read_to_string(path)?;
    let trimmed = content.trim_start();
    let rest = trimmed
        .strip_prefix("---")
        .context("missing agent frontmatter")?;
    let closing = rest
        .find("\n---")
        .context("unterminated agent frontmatter")?;
    let frontmatter: serde_yaml::Value = serde_yaml::from_str(&rest[..closing])?;
    let mapping = frontmatter
        .as_mapping()
        .context("agent frontmatter must be a mapping")?;
    let value = |key: &str| mapping.get(serde_yaml::Value::String(key.to_string()));
    let name = value("name")
        .and_then(serde_yaml::Value::as_str)
        .context("agent name is required")?
        .to_string();
    let description = value("description")
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or("Imported Claude agent")
        .to_string();
    let mut tools = Vec::new();
    if let Some(raw_tools) = value("tools") {
        let entries: Vec<&str> = match raw_tools {
            serde_yaml::Value::String(value) => value.split(',').collect(),
            serde_yaml::Value::Sequence(values) => values
                .iter()
                .filter_map(serde_yaml::Value::as_str)
                .collect(),
            _ => Vec::new(),
        };
        for entry in entries {
            let entry = entry.trim();
            let canonical = entry.split('(').next().unwrap_or_default();
            let supported = match canonical {
                "Read" | "Grep" | "Glob" | "Bash" => Some(canonical.to_string()),
                _ if entry.starts_with("mcp__") => Some(entry.to_string()),
                _ => None,
            };
            if let Some(supported) = supported
                && !tools.iter().any(|existing| existing == &supported)
            {
                tools.push(supported);
            }
        }
    }
    let mut normalized = serde_yaml::Mapping::new();
    normalized.insert("name".into(), name.clone().into());
    normalized.insert("description".into(), description.into());
    if !tools.is_empty() {
        normalized.insert(
            "tools".into(),
            serde_yaml::Value::Sequence(tools.into_iter().map(Into::into).collect()),
        );
    }
    let body = rest[closing + 4..].trim_start_matches(['\r', '\n']);
    let yaml = serde_yaml::to_string(&normalized)?;
    Ok((name, Some(format!("---\n{yaml}---\n\n{body}"))))
}

fn load_manifests(
    project_target: &Path,
    global_target: &Path,
) -> Result<HashMap<PathBuf, ProvenanceManifest>> {
    let mut manifests = HashMap::new();
    for root in [project_target, global_target] {
        let path = root.join(MANIFEST_NAME);
        let manifest = match fs::read_to_string(&path) {
            Ok(body) => serde_json::from_str(&body)
                .with_context(|| format!("invalid migration manifest at {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ProvenanceManifest::default()
            }
            Err(error) => return Err(error.into()),
        };
        manifests.insert(root.to_path_buf(), manifest);
    }
    Ok(manifests)
}

fn preflight(
    candidates: &[Candidate],
    manifests: &HashMap<PathBuf, ProvenanceManifest>,
    report: &mut MigrationReport,
) -> Result<Vec<Action>> {
    let mut destination_sources: HashMap<(&Path, &str), (&str, &str)> = HashMap::new();
    let mut actions = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let destination_key = (
            candidate.target_root.as_path(),
            candidate.target_key.as_str(),
        );
        if let Some((prior_source, prior_hash)) = destination_sources.get(&destination_key) {
            if *prior_hash != candidate.hash {
                report.collisions.push(format!(
                    "{}: competing sources {} and {}",
                    candidate.target.display(),
                    prior_source,
                    candidate.source.display()
                ));
            }
            actions.push(Action::Unchanged);
            continue;
        }
        destination_sources.insert(
            destination_key,
            (candidate.source_key.as_str(), candidate.hash.as_str()),
        );

        if !candidate.target.exists() {
            actions.push(Action::Copy);
            continue;
        }
        let manifest = manifests
            .get(&candidate.target_root)
            .expect("manifest initialized for each target root");
        let managed = manifest.entries.iter().find(|entry| {
            entry.target == candidate.target_key && entry.source == candidate.source_key
        });
        let Some(managed) = managed else {
            report.collisions.push(format!(
                "{}: existing destination is not managed from {}",
                candidate.target.display(),
                candidate.source.display()
            ));
            actions.push(Action::Unchanged);
            continue;
        };
        let target_hash = hash_path(&candidate.target)?;
        if target_hash == candidate.hash && managed.content_hash == candidate.hash {
            actions.push(Action::Unchanged);
        } else if target_hash != managed.content_hash {
            report.collisions.push(format!(
                "{}: managed destination was modified locally",
                candidate.target.display()
            ));
            actions.push(Action::Unchanged);
        } else {
            actions.push(Action::Update);
        }
    }
    Ok(actions)
}

fn promote_candidate(candidate: &Candidate, replace: bool) -> Result<()> {
    fs::create_dir_all(&candidate.target_root)?;
    let staging_root = candidate
        .target_root
        .join(format!(".migration-staging-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&staging_root)?;
    let staged = staging_root.join("resource");
    let result = (|| -> Result<()> {
        if let Some(content) = &candidate.adapted_content {
            fs::write(&staged, content)?;
        } else if candidate.source.is_dir() {
            copy_tree(&candidate.source, &staged)?;
        } else {
            fs::copy(&candidate.source, &staged)?;
        }
        if let Some(parent) = candidate.target.parent() {
            fs::create_dir_all(parent)?;
        }
        if replace {
            let backup = staging_root.join("backup");
            fs::rename(&candidate.target, &backup)?;
            if let Err(error) = fs::rename(&staged, &candidate.target) {
                let _ = fs::rename(&backup, &candidate.target);
                return Err(error.into());
            }
            remove_path(&backup)?;
        } else {
            fs::rename(&staged, &candidate.target)?;
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&staging_root);
    result
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir(target)?;
    let mut entries: Vec<_> = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if kind.is_symlink() {
            bail!(
                "refusing to copy symlink in skill directory: {}",
                entry.path().display()
            );
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), destination)?;
        } else {
            bail!("refusing to copy special file: {}", entry.path().display());
        }
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn hash_path(path: &Path) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    if path.is_file() {
        hasher.update(b"file\0");
        hasher.update(&fs::read(path)?);
    } else if path.is_dir() {
        let mut files = Vec::new();
        for entry in WalkDir::new(path).follow_links(false) {
            let entry = entry?;
            if entry.file_type().is_symlink() {
                bail!("symlink is not portable: {}", entry.path().display());
            }
            if entry.file_type().is_file() {
                files.push(entry.path().to_path_buf());
            }
        }
        files.sort();
        for file in files {
            let relative = file.strip_prefix(path)?;
            hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
            hasher.update(b"\0");
            hasher.update(&fs::read(file)?);
            hasher.update(b"\0");
        }
    } else {
        bail!("resource source does not exist: {}", path.display());
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn hash_file_bytes(bytes: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"file\0");
    hasher.update(bytes);
    hasher.finalize().to_hex().to_string()
}

fn upsert_manifest_entry(manifest: &mut ProvenanceManifest, candidate: &Candidate) {
    let entry = ProvenanceEntry {
        kind: candidate.kind,
        name: candidate.name.clone(),
        scope: candidate.scope,
        source: candidate.source_key.clone(),
        target: candidate.target_key.clone(),
        content_hash: candidate.hash.clone(),
        plugin: candidate.plugin.clone(),
        plugin_version: candidate.plugin_version.clone(),
    };
    if let Some(existing) = manifest.entries.iter_mut().find(|existing| {
        existing.target == candidate.target_key && existing.source == candidate.source_key
    }) {
        *existing = entry;
    } else {
        manifest.entries.push(entry);
    }
    manifest
        .entries
        .sort_by(|a, b| (&a.target, &a.source).cmp(&(&b.target, &b.source)));
}

fn write_manifests(manifests: &HashMap<PathBuf, ProvenanceManifest>) -> Result<()> {
    let ordered: BTreeMap<_, _> = manifests.iter().collect();
    for (root, manifest) in ordered {
        if manifest.entries.is_empty() {
            continue;
        }
        fs::create_dir_all(root)?;
        let target = root.join(MANIFEST_NAME);
        let staging = root.join(format!(
            ".migration-manifest-staging-{}",
            uuid::Uuid::new_v4()
        ));
        let body = serde_json::to_string_pretty(manifest)? + "\n";
        fs::write(&staging, body)?;
        if target.exists() {
            let backup = root.join(format!(
                ".migration-manifest-backup-{}",
                uuid::Uuid::new_v4()
            ));
            fs::rename(&target, &backup)?;
            if let Err(error) = fs::rename(&staging, &target) {
                let _ = fs::rename(&backup, &target);
                let _ = fs::remove_file(&staging);
                return Err(error.into());
            }
            fs::remove_file(backup)?;
        } else {
            fs::rename(staging, target)?;
        }
    }
    Ok(())
}

fn canonical_or_original(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Convert the already-resolved enabled plugin registry into migration inputs.
/// Registry conflict resolution selects one active plugin version, so stale
/// cache snapshots are never scanned or copied here.
pub fn enabled_plugin_resources(
    registry: &xai_grok_agent::plugins::PluginRegistry,
    project_root: &Path,
) -> Vec<PluginResources> {
    let mut seen = HashSet::new();
    registry
        .enabled_plugins()
        .into_iter()
        .filter(|plugin| seen.insert(plugin.name.clone()))
        .map(|plugin| PluginResources {
            name: plugin.name.clone(),
            version: plugin.version.clone(),
            scope: if plugin.root.starts_with(project_root) {
                MigrationScope::Project
            } else {
                MigrationScope::Global
            },
            root: plugin.root.clone(),
            skill_dirs: plugin.skill_dirs.clone(),
            agent_dirs: plugin.agent_dirs.clone(),
        })
        .collect()
}

/// Run migration using the current directory, user home, and the effective
/// enabled plugin registry. This path reads resource/config metadata only; it
/// does not open authentication or provider credential stores.
pub fn migrate_discovered_resources(cwd: &Path, dry_run: bool) -> Result<MigrationReport> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot migrate resources without a user home"))?;
    let project_root = crate::claude_import::find_project_root(cwd);
    let trust_store = xai_grok_agent::plugins::TrustStore::load();
    let mut plugins: crate::agent::config::PluginsConfig =
        crate::config::load_effective_config_disk_only()
            .ok()
            .and_then(|root| {
                root.get("plugins")
                    .and_then(|value| value.clone().try_into().ok())
            })
            .unwrap_or_default();
    plugins.merge_claude_enabled_plugins(Some(cwd));
    let mut discovery = plugins.to_discovery_config();
    let discovered =
        xai_grok_agent::plugins::discover_plugins(Some(cwd), &discovery, &trust_store, false);
    discovery.populate_plugin_lists(&discovered);
    let registry = xai_grok_agent::plugins::PluginRegistry::from_discovered(
        discovered,
        &discovery.disabled,
        &discovery.enabled,
    );
    migrate_resources(&MigrationOptions {
        cwd: cwd.to_path_buf(),
        home,
        dry_run,
        plugin_resources: enabled_plugin_resources(&registry, &project_root),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, body: &str) -> String {
        format!("---\nname: {name}\ndescription: test\n---\n{body}\n")
    }

    fn agent(name: &str, body: &str) -> String {
        format!("---\nname: {name}\ndescription: test\n---\n{body}\n")
    }

    fn fixture() -> (tempfile::TempDir, MigrationOptions) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&project).unwrap();
        let options = MigrationOptions {
            cwd: project,
            home,
            dry_run: false,
            plugin_resources: Vec::new(),
        };
        (temp, options)
    }

    #[test]
    fn dry_run_preserves_scope_and_writes_nothing() {
        let (_temp, mut options) = fixture();
        let global_skill = options.home.join(".claude/skills/global-one");
        let project_agent = options.cwd.join(".codex/agents");
        fs::create_dir_all(&global_skill).unwrap();
        fs::create_dir_all(&project_agent).unwrap();
        fs::write(global_skill.join("SKILL.md"), skill("global-one", "body")).unwrap();
        fs::write(project_agent.join("review.md"), agent("review", "prompt")).unwrap();
        options.dry_run = true;

        let report = migrate_resources(&options).unwrap();

        assert_eq!(report.copied, 2);
        assert!(!options.home.join(".bandicot").exists());
        assert!(!options.cwd.join(".bandicot").exists());
    }

    #[test]
    fn copies_whole_skill_and_agent_then_is_idempotent() {
        let (_temp, options) = fixture();
        let skill_dir = options.cwd.join(".claude/skills/review");
        let agent_dir = options.home.join(".codex/agents");
        fs::create_dir_all(skill_dir.join("references")).unwrap();
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), skill("review", "body")).unwrap();
        fs::write(skill_dir.join("references/check.md"), "check").unwrap();
        fs::write(agent_dir.join("helper.md"), agent("helper", "prompt")).unwrap();

        let first = migrate_resources(&options).unwrap();
        let second = migrate_resources(&options).unwrap();

        assert_eq!(first.copied, 2);
        assert_eq!(second.unchanged, 2);
        assert_eq!(
            fs::read_to_string(
                options
                    .cwd
                    .join(".bandicot/skills/review/references/check.md")
            )
            .unwrap(),
            "check"
        );
        assert!(options.home.join(".bandicot/agents/helper.md").is_file());
    }

    #[test]
    fn refuses_unmanaged_overwrite_without_touching_source_or_target() {
        let (_temp, options) = fixture();
        let source = options.cwd.join(".claude/skills/review");
        let target = options.cwd.join(".bandicot/skills/review");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("SKILL.md"), skill("review", "source")).unwrap();
        fs::write(target.join("SKILL.md"), skill("review", "unmanaged")).unwrap();

        let error = migrate_resources(&options).unwrap_err().to_string();

        assert!(error.contains("not managed"));
        assert!(
            fs::read_to_string(source.join("SKILL.md"))
                .unwrap()
                .contains("source")
        );
        assert!(
            fs::read_to_string(target.join("SKILL.md"))
                .unwrap()
                .contains("unmanaged")
        );
    }

    #[test]
    fn updates_only_managed_unchanged_destination() {
        let (_temp, options) = fixture();
        let source = options.cwd.join(".claude/skills/review");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("SKILL.md"), skill("review", "one")).unwrap();
        migrate_resources(&options).unwrap();
        fs::write(source.join("SKILL.md"), skill("review", "two")).unwrap();

        let report = migrate_resources(&options).unwrap();

        assert_eq!(report.updated, 1);
        assert!(
            fs::read_to_string(options.cwd.join(".bandicot/skills/review/SKILL.md"))
                .unwrap()
                .contains("two")
        );
    }

    #[test]
    fn excludes_unlisted_stale_plugin_versions() {
        let (_temp, mut options) = fixture();
        let cache = options.home.join(".claude/plugins/cache/example");
        let old = cache.join("1.0.0/skills/tool");
        let current_root = cache.join("2.0.0");
        let current = current_root.join("skills/tool");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&current).unwrap();
        fs::write(old.join("SKILL.md"), skill("old-tool", "old")).unwrap();
        fs::write(current.join("SKILL.md"), skill("tool", "current")).unwrap();
        options.plugin_resources.push(PluginResources {
            name: "example".into(),
            version: Some("2.0.0".into()),
            scope: MigrationScope::Global,
            root: current_root,
            skill_dirs: vec![current],
            agent_dirs: vec![],
        });

        migrate_resources(&options).unwrap();

        assert!(options.home.join(".bandicot/skills/tool").is_dir());
        assert!(!options.home.join(".bandicot/skills/old-tool").exists());
        let manifest =
            fs::read_to_string(options.home.join(".bandicot").join(MANIFEST_NAME)).unwrap();
        assert!(manifest.contains("2.0.0"));
        assert!(!manifest.contains("old\n"));
    }
}
