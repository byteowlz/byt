use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use config::{Config, Environment, File, FileFormat};
use env_logger::fmt::WriteStyle;
use log::{LevelFilter, debug, info};
use serde::{Deserialize, Serialize};

const APP_NAME: &str = env!("CARGO_PKG_NAME");

fn main() {
    if let Err(err) = try_main() {
        let _ = writeln!(io::stderr(), "{err:?}");
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    let cli = Cli::parse();

    let ctx = RuntimeContext::new(cli.common.clone())?;
    ctx.init_logging()?;
    debug!("resolved paths: {:#?}", ctx.paths);

    match cli.command {
        Command::Catalog { command } => handle_catalog(&ctx, command),
        Command::Lint(cmd) => handle_lint(&ctx, cmd),
        Command::Status(cmd) => handle_status(&ctx, cmd),
        Command::Ready => handle_ready(&ctx),
        Command::Triage(cmd) => handle_triage(&ctx, cmd),
        Command::Search(cmd) => handle_search(&ctx, cmd),
        Command::Memory { command } => handle_memory(&ctx, command),
        Command::Sync { command } => handle_sync(&ctx, command),
        Command::Init(cmd) => handle_init(&ctx, cmd),
        Command::Config { command } => handle_config(&ctx, command),
        Command::Secrets { command } => handle_secrets(&ctx, command),
        Command::New(cmd) => handle_new(&ctx, cmd),
        Command::Schema { command } => handle_schema(&ctx, command),
        Command::Completions { shell } => handle_completions(shell),
    }
}

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Byteowlz meta-tool for cross-repo management and governance",
    propagate_version = true
)]
struct Cli {
    #[command(flatten)]
    common: CommonOpts,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Args)]
struct CommonOpts {
    /// Override the config file path
    #[arg(long, value_name = "PATH", global = true)]
    config: Option<PathBuf>,
    /// Override the workspace root directory
    #[arg(long, value_name = "PATH", global = true, env = "BYT_WORKSPACE")]
    workspace: Option<PathBuf>,
    /// Reduce output to only errors
    #[arg(short, long, action = clap::ArgAction::SetTrue, global = true)]
    quiet: bool,
    /// Increase logging verbosity (stackable)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    /// Enable debug logging
    #[arg(long, global = true)]
    debug: bool,
    /// Output machine readable JSON
    #[arg(long, global = true)]
    json: bool,
    /// Do not change anything on disk
    #[arg(long = "dry-run", global = true)]
    dry_run: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ColorOption {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Manage the project catalog
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
    /// Check governance compliance across repos
    Lint(LintCommand),
    /// Show status of all repositories
    Status(StatusCommand),
    /// Show ready work from govnr-level beads
    Ready,
    /// Cross-repo triage and issue management (via bv workspace)
    Triage(TriageCommand),
    /// Search agent conversation history (via cass)
    Search(SearchCommand),
    /// Manage memories (via mmry)
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Sync memories across machines via git
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
    /// Initialize byt configuration
    Init(InitCommand),
    /// Inspect and manage configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Manage GitHub repository secrets for releases
    Secrets {
        #[command(subcommand)]
        command: SecretsCommand,
    },
    /// Create a new project from template
    New(NewCommand),
    /// Manage JSON schemas (sync to central schemas repo)
    Schema {
        #[command(subcommand)]
        command: SchemaCommand,
    },
    /// Generate shell completions
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
enum CatalogCommand {
    /// Refresh the catalog by scanning all repositories
    Refresh {
        /// Include git commit dates and beads issue counts (slower)
        #[arg(long)]
        full: bool,
    },
    /// Show the current catalog
    Show,
    /// List all repositories
    List,
}

#[derive(Debug, Clone, Args)]
struct LintCommand {
    /// Only check specific repositories
    #[arg(value_name = "REPO")]
    repos: Vec<String>,
    /// Fix issues automatically where possible
    #[arg(long)]
    fix: bool,
}

#[derive(Debug, Clone, Args)]
struct StatusCommand {
    /// Only show repos with issues
    #[arg(long)]
    issues_only: bool,
}

#[derive(Debug, Clone, Args)]
struct TriageCommand {
    /// Regenerate workspace.yaml before running
    #[arg(long)]
    refresh: bool,
    /// Show only next recommended item
    #[arg(long)]
    next: bool,
    /// Show execution plan
    #[arg(long)]
    plan: bool,
    /// Show insights
    #[arg(long)]
    insights: bool,
}

#[derive(Debug, Clone, Args)]
struct SearchCommand {
    /// Search query
    query: String,
    /// Filter by workspace/repo name
    #[arg(long, short = 'r')]
    repo: Option<String>,
    /// Limit results
    #[arg(long, short = 'l', default_value = "10")]
    limit: usize,
    /// Filter to last N days
    #[arg(long)]
    days: Option<u32>,
}

#[derive(Debug, Subcommand)]
enum MemoryCommand {
    /// Add a memory
    Add {
        /// Memory content
        content: String,
        /// Project/repo to associate memory with (auto-detected from cwd, or specify explicitly)
        #[arg(long, short = 'p')]
        project: Option<String>,
        /// Force govnr (cross-repo) store even when inside a repo
        #[arg(long, short = 'g')]
        govnr: bool,
        /// Category
        #[arg(long, short = 'c')]
        category: Option<String>,
        /// Tags (comma-separated)
        #[arg(long, short = 't')]
        tags: Option<String>,
        /// Importance (1-10)
        #[arg(long, short = 'i', default_value = "5")]
        importance: u8,
    },
    /// Search memories
    Search {
        /// Search query
        query: String,
        /// Project/repo to search in (auto-detected from cwd, or specify explicitly)
        #[arg(long, short = 'p')]
        project: Option<String>,
        /// Search govnr (cross-repo) store
        #[arg(long, short = 'g')]
        govnr: bool,
        /// Search all projects/stores
        #[arg(long, short = 'a')]
        all: bool,
        /// Limit results
        #[arg(long, short = 'l', default_value = "10")]
        limit: usize,
    },
    /// List available projects (from catalog)
    Projects,
}

#[derive(Debug, Subcommand)]
enum SyncCommand {
    /// Export memories to .sync/ for git-based sync
    Push {
        /// Only sync specific stores (default: govnr + local repos)
        #[arg(long, short = 's')]
        stores: Vec<String>,
    },
    /// Import memories from .sync/ after git pull
    Pull {
        /// Only sync specific stores (default: govnr + local repos)
        #[arg(long, short = 's')]
        stores: Vec<String>,
    },
    /// Show sync status (what would be synced)
    Status,
}

#[derive(Debug, Clone, Args)]
struct InitCommand {
    /// Recreate configuration even if it already exists
    #[arg(long = "force")]
    force: bool,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Output the effective configuration
    Show,
    /// Print the resolved config file path
    Path,
}

#[derive(Debug, Subcommand)]
enum SecretsCommand {
    /// Set up all release secrets for a repository
    Setup {
        /// Repository name (e.g., mmry) or full path (byteowlz/mmry)
        repo: String,
        /// Skip AUR secrets (only set TAP_GITHUB_TOKEN)
        #[arg(long)]
        skip_aur: bool,
    },
    /// List secrets for a repository
    List {
        /// Repository name (e.g., mmry) or full path (byteowlz/mmry)
        repo: String,
    },
    /// Set a specific secret
    Set {
        /// Repository name (e.g., mmry) or full path (byteowlz/mmry)
        repo: String,
        /// Secret name
        name: String,
        /// Secret value (will prompt if not provided)
        #[arg(long)]
        value: Option<String>,
        /// Read value from file
        #[arg(long, conflicts_with = "value")]
        from_file: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Args)]
struct NewCommand {
    /// Project name
    name: String,
    /// Template to use (rust-cli, rust-workspace, python-cli, go-cli)
    #[arg(short, long, default_value = "rust-cli")]
    template: String,
    /// Create GitHub repository
    #[arg(long)]
    github: bool,
    /// Make GitHub repo private
    #[arg(long)]
    private: bool,
    /// Project description
    #[arg(short, long)]
    description: Option<String>,
    /// Directory to create project in (default: current directory)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum SchemaCommand {
    /// Check for updated schemas across all repos
    Check,
    /// Sync updated schemas to the central schemas repository
    Sync {
        /// Commit and push changes
        #[arg(long)]
        push: bool,
        /// Only sync specific repos
        repos: Vec<String>,
    },
    /// List all schemas in the workspace
    List,
}

// ============================================================================
// Catalog Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Catalog {
    #[serde(rename = "$schema")]
    schema: Option<String>,
    generated: DateTime<Utc>,
    workspace: String,
    repos: HashMap<String, RepoInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RepoInfo {
    path: String,
    description: Option<String>,
    languages: Vec<String>,
    status: RepoStatus,
    has_beads: bool,
    has_justfile: bool,
    has_agents_md: bool,
    last_commit: Option<String>,
    open_issues: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum RepoStatus {
    Active,
    Stale,
    Archived,
    Unknown,
}

// ============================================================================
// Lint Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LintReport {
    passed: bool,
    repos_checked: usize,
    issues: Vec<LintIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LintIssue {
    repo: String,
    rule: String,
    severity: Severity,
    message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Error,
    Warning,
    Info,
}

// ============================================================================
// Runtime Context
// ============================================================================

#[derive(Debug, Clone)]
struct RuntimeContext {
    common: CommonOpts,
    paths: AppPaths,
    config: AppConfig,
}

impl RuntimeContext {
    fn new(common: CommonOpts) -> Result<Self> {
        let mut paths = AppPaths::discover(common.config.clone(), common.workspace.clone())?;
        let config = load_config(&paths)?;
        
        // Apply workspace override from config if not already set via CLI
        if common.workspace.is_none() {
            if let Some(ref ws) = config.workspace {
                if let Ok(expanded) = expand_path(PathBuf::from(ws)) {
                    paths.workspace_root = expanded;
                }
            }
        }
        
        Ok(Self {
            common,
            paths,
            config,
        })
    }

    fn init_logging(&self) -> Result<()> {
        if self.common.quiet {
            log::set_max_level(LevelFilter::Off);
            return Ok(());
        }

        let mut builder =
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));

        builder.filter_level(self.effective_log_level());

        let disable_color = !io::stderr().is_terminal();
        if disable_color {
            builder.write_style(WriteStyle::Never);
        }

        builder.try_init().or_else(|_| Ok(()))
    }

    fn effective_log_level(&self) -> LevelFilter {
        if self.common.debug {
            LevelFilter::Debug
        } else {
            match self.common.verbose {
                0 => LevelFilter::Info,
                1 => LevelFilter::Debug,
                _ => LevelFilter::Trace,
            }
        }
    }

    fn workspace_root(&self) -> &Path {
        &self.paths.workspace_root
    }

    fn catalog_path(&self) -> PathBuf {
        if let Some(ref path) = self.config.catalog_path {
            let expanded = expand_path(PathBuf::from(path)).unwrap_or_else(|_| PathBuf::from(path));
            if expanded.is_absolute() {
                expanded
            } else {
                self.paths.workspace_root.join(expanded)
            }
        } else {
            // Default: look in govnr/ subdirectory
            self.paths.workspace_root.join("govnr").join("CATALOG.json")
        }
    }
}

#[derive(Debug, Clone)]
struct AppPaths {
    config_file: PathBuf,
    workspace_root: PathBuf,
}

impl AppPaths {
    fn discover(config_override: Option<PathBuf>, workspace_override: Option<PathBuf>) -> Result<Self> {
        let config_file = match config_override {
            Some(path) => expand_path(path)?,
            None => default_config_dir()?.join("config.toml"),
        };

        let workspace_root = match workspace_override {
            Some(path) => expand_path(path)?,
            None => discover_workspace_root()?,
        };

        Ok(Self {
            config_file,
            workspace_root,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct AppConfig {
    /// Workspace root directory (auto-detected if not set)
    workspace: Option<String>,
    /// Path to CATALOG.json (relative to workspace or absolute)
    catalog_path: Option<String>,
    /// Directories to ignore when scanning for repos
    ignore_dirs: Vec<String>,
    /// Required files for governance compliance
    required_files: Vec<String>,
    /// Release and distribution settings
    release: ReleaseConfig,
    /// Template settings
    templates: TemplatesConfig,
    /// Schema settings
    schemas: SchemaConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct TemplatesConfig {
    /// GitHub repository containing templates (e.g., "byteowlz/templates")
    repo: String,
    /// Branch to use (default: main)
    branch: String,
}

impl Default for TemplatesConfig {
    fn default() -> Self {
        Self {
            repo: "byteowlz/templates".to_string(),
            branch: "main".to_string(),
        }
    }
}

/// Template manifest for composition support
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TemplateManifest {
    /// Template metadata
    #[serde(default)]
    template: TemplateMetadata,
    /// Templates to compose into this one
    #[serde(default)]
    compose: Vec<TemplateComposition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TemplateMetadata {
    /// Template name
    #[serde(default)]
    name: String,
    /// Template description
    #[serde(default)]
    description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TemplateComposition {
    /// Source template name to pull from
    source: String,
    /// Target directory within this template (where to place the composed template)
    target: String,
    /// Optional prefix to replace with project name (e.g., "rust-" becomes "myproject-")
    #[serde(default)]
    rename_prefix: Option<String>,
    /// Files/directories to exclude from the composed template
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct SchemaConfig {
    /// Path to schemas repository (relative to workspace or absolute)
    repo_path: String,
    /// Schema file pattern to look for in repos (e.g., "examples/*.schema.json")
    patterns: Vec<String>,
    /// Base URL for schema references (used in $schema fields)
    base_url: String,
}

impl Default for SchemaConfig {
    fn default() -> Self {
        Self {
            repo_path: "schemas".to_string(),
            patterns: vec![
                "examples/*.schema.json".to_string(),
                "schemas/*.schema.json".to_string(),
            ],
            base_url: "https://raw.githubusercontent.com/byteowlz/schemas/refs/heads/main".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct ReleaseConfig {
    /// GitHub organization/user (used as default when repo has no org prefix)
    github_org: String,
    /// Homebrew tap repository (e.g., "myorg/homebrew-tap")
    homebrew_tap: String,
    /// Scoop bucket repository (e.g., "myorg/scoop-bucket")
    scoop_bucket: String,
    /// AUR account email for commits
    aur_email: Option<String>,
    /// Path to AUR SSH private key
    aur_ssh_key_path: Option<String>,
    /// Name of the GitHub PAT secret for tap/bucket updates (default: TAP_GITHUB_TOKEN)
    tap_token_name: String,
}

impl Default for ReleaseConfig {
    fn default() -> Self {
        Self {
            github_org: String::new(), // Must be configured
            homebrew_tap: String::new(),
            scoop_bucket: String::new(),
            aur_email: None,
            aur_ssh_key_path: None,
            tap_token_name: "TAP_GITHUB_TOKEN".to_string(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            workspace: None,
            catalog_path: None,
            ignore_dirs: vec![
                ".git".to_string(),
                "node_modules".to_string(),
                "target".to_string(),
                "dist".to_string(),
                ".venv".to_string(),
                "__pycache__".to_string(),
            ],
            required_files: vec![
                "justfile".to_string(),
                ".beads".to_string(),
            ],
            release: ReleaseConfig::default(),
            templates: TemplatesConfig::default(),
            schemas: SchemaConfig::default(),
        }
    }
}

// ============================================================================
// Command Handlers
// ============================================================================

fn handle_catalog(ctx: &RuntimeContext, command: CatalogCommand) -> Result<()> {
    match command {
        CatalogCommand::Refresh { full } => refresh_catalog(ctx, full),
        CatalogCommand::Show => show_catalog(ctx),
        CatalogCommand::List => list_repos(ctx),
    }
}

fn refresh_catalog(ctx: &RuntimeContext, full: bool) -> Result<()> {
    info!("Scanning workspace: {}", ctx.workspace_root().display());

    let repos = scan_repositories(ctx, full)?;
    let catalog = Catalog {
        schema: Some("./catalog.schema.json".to_string()),
        generated: Utc::now(),
        workspace: ctx.workspace_root().display().to_string(),
        repos,
    };

    if ctx.common.dry_run {
        info!("dry-run: would write catalog to {}", ctx.catalog_path().display());
        if ctx.common.json {
            println!("{}", serde_json::to_string_pretty(&catalog)?);
        }
        return Ok(());
    }

    let json = serde_json::to_string_pretty(&catalog)?;
    fs::write(ctx.catalog_path(), &json)
        .with_context(|| format!("writing catalog to {}", ctx.catalog_path().display()))?;

    if ctx.common.json {
        println!("{}", json);
    } else {
        println!("Catalog refreshed: {} repositories found", catalog.repos.len());
        println!("Written to: {}", ctx.catalog_path().display());
    }

    Ok(())
}

fn show_catalog(ctx: &RuntimeContext) -> Result<()> {
    let catalog_path = ctx.catalog_path();
    if !catalog_path.exists() {
        return Err(anyhow!("Catalog not found. Run 'byt catalog refresh' first."));
    }

    let content = fs::read_to_string(&catalog_path)?;
    let catalog: Catalog = serde_json::from_str(&content)?;

    if ctx.common.json {
        println!("{}", serde_json::to_string_pretty(&catalog)?);
    } else {
        println!("Byteowlz Catalog");
        println!("================");
        println!("Generated: {}", catalog.generated);
        println!("Repositories: {}", catalog.repos.len());
        println!();

        for (name, info) in &catalog.repos {
            let status_icon = match info.status {
                RepoStatus::Active => "[active]",
                RepoStatus::Stale => "[stale]",
                RepoStatus::Archived => "[archived]",
                RepoStatus::Unknown => "[?]",
            };
            let langs = if info.languages.is_empty() {
                String::new()
            } else {
                format!(" ({})", info.languages.join(", "))
            };
            println!("  {} {}{}", status_icon, name, langs);
            if let Some(ref desc) = info.description {
                println!("      {}", desc);
            }
        }
    }

    Ok(())
}

fn list_repos(ctx: &RuntimeContext) -> Result<()> {
    let repos = scan_repositories(ctx, false)?;

    if ctx.common.json {
        let names: Vec<&String> = repos.keys().collect();
        println!("{}", serde_json::to_string_pretty(&names)?);
    } else {
        for name in repos.keys() {
            println!("{}", name);
        }
    }

    Ok(())
}

fn scan_repositories(ctx: &RuntimeContext, full: bool) -> Result<HashMap<String, RepoInfo>> {
    let mut repos = HashMap::new();
    let root = ctx.workspace_root();

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip hidden directories and known non-repo dirs
        if name.starts_with('.') || name.starts_with('_') {
            continue;
        }

        // Skip directories in ignore list
        if ctx.config.ignore_dirs.contains(&name) {
            continue;
        }

        // Check if it's a git repo or has markers of a project
        let is_git_repo = path.join(".git").exists();
        let has_cargo = path.join("Cargo.toml").exists();
        let has_package_json = path.join("package.json").exists();
        let has_pyproject = path.join("pyproject.toml").exists();
        let has_go_mod = path.join("go.mod").exists();

        if !is_git_repo && !has_cargo && !has_package_json && !has_pyproject && !has_go_mod {
            continue;
        }

        let info = analyze_repo(&path, &name, full)?;
        repos.insert(name, info);
    }

    Ok(repos)
}

fn analyze_repo(path: &Path, name: &str, full: bool) -> Result<RepoInfo> {
    let has_beads = path.join(".beads").exists();
    let has_justfile = path.join("justfile").exists() || path.join("Justfile").exists();
    let has_agents_md = path.join("AGENTS.md").exists();

    // Detect languages
    let mut languages = Vec::new();
    if path.join("Cargo.toml").exists() {
        languages.push("rust".to_string());
    }
    if path.join("package.json").exists() {
        // Check for TypeScript
        if path.join("tsconfig.json").exists() {
            languages.push("typescript".to_string());
        } else {
            languages.push("javascript".to_string());
        }
    }
    if path.join("pyproject.toml").exists() || path.join("setup.py").exists() {
        languages.push("python".to_string());
    }
    if path.join("go.mod").exists() {
        languages.push("go".to_string());
    }

    // Check for Tauri (cross-platform app)
    if path.join("src-tauri").exists() {
        languages.push("tauri".to_string());
    }

    // Get description from various sources
    let description = get_repo_description(path);

    // Only fetch slow data (git, beads) if --full is specified
    let (last_commit, status, open_issues) = if full {
        let last_commit = get_last_commit_date(path);
        
        let status = match &last_commit {
            Some(date) => {
                if let Ok(commit_date) = DateTime::parse_from_rfc3339(date) {
                    let days_old = (Utc::now() - commit_date.with_timezone(&Utc)).num_days();
                    if days_old > 180 {
                        RepoStatus::Stale
                    } else {
                        RepoStatus::Active
                    }
                } else {
                    RepoStatus::Unknown
                }
            }
            None => RepoStatus::Unknown,
        };

        let open_issues = if has_beads {
            get_beads_open_count(path)
        } else {
            None
        };

        (last_commit, status, open_issues)
    } else {
        (None, RepoStatus::Unknown, None)
    };

    Ok(RepoInfo {
        path: name.to_string(),
        description,
        languages,
        status,
        has_beads,
        has_justfile,
        has_agents_md,
        last_commit,
        open_issues,
    })
}

fn get_repo_description(path: &Path) -> Option<String> {
    // Try to get from Cargo.toml
    if let Ok(content) = fs::read_to_string(path.join("Cargo.toml")) {
        if let Ok(cargo) = content.parse::<toml::Table>() {
            if let Some(pkg) = cargo.get("package").and_then(|p| p.as_table()) {
                if let Some(desc) = pkg.get("description").and_then(|d| d.as_str()) {
                    return Some(desc.to_string());
                }
            }
        }
    }

    // Try to get from package.json
    if let Ok(content) = fs::read_to_string(path.join("package.json")) {
        if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(desc) = pkg.get("description").and_then(|d| d.as_str()) {
                return Some(desc.to_string());
            }
        }
    }

    // Try to get from pyproject.toml
    if let Ok(content) = fs::read_to_string(path.join("pyproject.toml")) {
        if let Ok(pyproject) = content.parse::<toml::Table>() {
            if let Some(project) = pyproject.get("project").and_then(|p| p.as_table()) {
                if let Some(desc) = project.get("description").and_then(|d| d.as_str()) {
                    return Some(desc.to_string());
                }
            }
        }
    }

    None
}

fn get_last_commit_date(path: &Path) -> Option<String> {
    use std::process::Command;

    let output = Command::new("git")
        .args(["log", "-1", "--format=%aI"])
        .current_dir(path)
        .output()
        .ok()?;

    if output.status.success() {
        let date = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !date.is_empty() {
            return Some(date);
        }
    }

    None
}

fn get_beads_open_count(path: &Path) -> Option<u32> {
    use std::process::Command;

    let output = Command::new("bd")
        .args(["list", "--status", "open", "--json"])
        .current_dir(path)
        .output()
        .ok()?;

    if output.status.success() {
        let content = String::from_utf8_lossy(&output.stdout);
        if let Ok(issues) = serde_json::from_str::<Vec<serde_json::Value>>(&content) {
            return Some(issues.len() as u32);
        }
    }

    None
}

fn handle_lint(ctx: &RuntimeContext, cmd: LintCommand) -> Result<()> {
    info!("Linting workspace: {}", ctx.workspace_root().display());

    let repos = if cmd.repos.is_empty() {
        scan_repositories(ctx, false)?
    } else {
        let all_repos = scan_repositories(ctx, false)?;
        all_repos
            .into_iter()
            .filter(|(name, _)| cmd.repos.contains(name))
            .collect()
    };

    let mut issues = Vec::new();

    for (name, info) in &repos {
        // Rule: Must have justfile
        if !info.has_justfile {
            issues.push(LintIssue {
                repo: name.clone(),
                rule: "missing-justfile".to_string(),
                severity: Severity::Warning,
                message: "Repository should have a justfile for standardized commands".to_string(),
            });
        }

        // Rule: Must have .beads for issue tracking
        if !info.has_beads {
            issues.push(LintIssue {
                repo: name.clone(),
                rule: "missing-beads".to_string(),
                severity: Severity::Warning,
                message: "Repository should use bd (beads) for issue tracking".to_string(),
            });
        }

        // Rule: Should have AGENTS.md
        if !info.has_agents_md {
            issues.push(LintIssue {
                repo: name.clone(),
                rule: "missing-agents-md".to_string(),
                severity: Severity::Info,
                message: "Repository should have an AGENTS.md file for AI instructions".to_string(),
            });
        }

        // Rule: Stale repos should be reviewed
        if info.status == RepoStatus::Stale {
            issues.push(LintIssue {
                repo: name.clone(),
                rule: "stale-repo".to_string(),
                severity: Severity::Info,
                message: "Repository has not been updated in over 180 days".to_string(),
            });
        }
    }

    let report = LintReport {
        passed: issues.iter().all(|i| !matches!(i.severity, Severity::Error)),
        repos_checked: repos.len(),
        issues,
    };

    if ctx.common.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Lint Report");
        println!("===========");
        println!("Repositories checked: {}", report.repos_checked);
        println!("Issues found: {}", report.issues.len());
        println!();

        if report.issues.is_empty() {
            println!("All checks passed!");
        } else {
            for issue in &report.issues {
                let severity_icon = match issue.severity {
                    Severity::Error => "ERROR",
                    Severity::Warning => "WARN",
                    Severity::Info => "INFO",
                };
                println!("[{}] {} - {}: {}", severity_icon, issue.repo, issue.rule, issue.message);
            }
        }
    }

    Ok(())
}

fn handle_status(ctx: &RuntimeContext, cmd: StatusCommand) -> Result<()> {
    let repos = scan_repositories(ctx, false)?;

    if ctx.common.json {
        println!("{}", serde_json::to_string_pretty(&repos)?);
        return Ok(());
    }

    println!("Repository Status");
    println!("=================");
    println!();

    let mut active = 0;
    let mut stale = 0;

    for (name, info) in &repos {
        if cmd.issues_only && info.open_issues.unwrap_or(0) == 0 {
            continue;
        }

        match info.status {
            RepoStatus::Active => active += 1,
            RepoStatus::Stale => stale += 1,
            _ => {}
        }

        let status_icon = match info.status {
            RepoStatus::Active => "+",
            RepoStatus::Stale => "~",
            RepoStatus::Archived => "-",
            RepoStatus::Unknown => "?",
        };

        let compliance = format!(
            "{}{}{}",
            if info.has_justfile { "J" } else { "-" },
            if info.has_beads { "B" } else { "-" },
            if info.has_agents_md { "A" } else { "-" }
        );

        let issues = info.open_issues
            .map(|n| format!(" ({} issues)", n))
            .unwrap_or_default();

        println!("[{}] {} [{}]{}", status_icon, name, compliance, issues);
    }

    println!();
    println!("Legend: J=justfile, B=beads, A=AGENTS.md");
    println!("Active: {}, Stale: {}, Total: {}", active, stale, repos.len());

    Ok(())
}

fn handle_ready(ctx: &RuntimeContext) -> Result<()> {
    use std::process::Command;

    // Use bd ready from the govnr workspace (not scanning all repos)
    let output = Command::new("bd")
        .args(["ready", "--json"])
        .current_dir(ctx.workspace_root())
        .output()
        .context("running bd ready")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("bd ready failed: {}", stderr));
    }

    let content = String::from_utf8_lossy(&output.stdout);
    
    if ctx.common.json {
        // Pass through JSON directly
        println!("{}", content);
    } else {
        let issues: Vec<serde_json::Value> = serde_json::from_str(&content)
            .unwrap_or_default();
        
        if issues.is_empty() {
            println!("No ready work found at govnr level.");
            println!("Tip: Use 'bd ready' in individual repos for repo-specific work.");
        } else {
            println!("Ready Work (Govnr Level)");
            println!("========================");
            for issue in &issues {
                let id = issue.get("id").and_then(|i| i.as_str()).unwrap_or("?");
                let title = issue.get("title").and_then(|t| t.as_str()).unwrap_or("?");
                let priority = issue.get("priority").and_then(|p| p.as_i64()).unwrap_or(0);
                let issue_type = issue.get("issue_type").and_then(|t| t.as_str()).unwrap_or("task");
                println!("[P{}] {} ({}): {}", priority, id, issue_type, title);
            }
        }
    }

    Ok(())
}

fn handle_triage(ctx: &RuntimeContext, cmd: TriageCommand) -> Result<()> {
    use std::process::Command;

    let workspace_config = ctx.workspace_root().join(".bv/workspace.yaml");
    
    // Regenerate workspace.yaml if requested or if it doesn't exist
    if cmd.refresh || !workspace_config.exists() {
        generate_workspace_config(ctx)?;
    }

    // Determine which bv command to run
    let bv_flag = if cmd.next {
        "-robot-next"
    } else if cmd.plan {
        "-robot-plan"
    } else if cmd.insights {
        "-robot-insights"
    } else {
        "-robot-triage"
    };

    let output = Command::new("bv")
        .args(["-workspace", &workspace_config.display().to_string(), bv_flag])
        .current_dir(ctx.workspace_root())
        .output()
        .context("running bv - is bv installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("bv failed: {}", stderr));
    }

    let content = String::from_utf8_lossy(&output.stdout);
    
    if ctx.common.json {
        println!("{}", content);
    } else {
        // Parse and display human-friendly output
        if let Ok(triage) = serde_json::from_str::<serde_json::Value>(&content) {
            display_triage(&triage, cmd.next);
        } else {
            println!("{}", content);
        }
    }

    Ok(())
}

fn generate_workspace_config(ctx: &RuntimeContext) -> Result<()> {
    let repos = scan_repositories(ctx, false)?;
    let bv_dir = ctx.workspace_root().join(".bv");
    
    fs::create_dir_all(&bv_dir)?;
    
    let mut yaml = String::new();
    yaml.push_str("# Byteowlz workspace configuration for bv\n");
    yaml.push_str("# Auto-generated by byt - do not edit manually\n");
    yaml.push_str("# Regenerate with: byt triage --refresh\n\n");
    yaml.push_str("repos:\n");
    
    for (name, info) in &repos {
        if info.has_beads {
            yaml.push_str(&format!("  - path: ./{}\n", name));
            yaml.push_str(&format!("    name: {}\n", name));
        }
    }
    
    let config_path = bv_dir.join("workspace.yaml");
    fs::write(&config_path, yaml)?;
    info!("Generated workspace config: {}", config_path.display());
    
    Ok(())
}

fn display_triage(triage: &serde_json::Value, next_only: bool) {
    if next_only {
        // Display single next item
        if let Some(id) = triage.get("id").and_then(|v| v.as_str()) {
            let title = triage.get("title").and_then(|v| v.as_str()).unwrap_or("?");
            let score = triage.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            println!("Next recommended: {} (score: {:.2})", id, score);
            println!("  {}", title);
            if let Some(reasons) = triage.get("reasons").and_then(|v| v.as_array()) {
                for reason in reasons {
                    if let Some(r) = reason.as_str() {
                        println!("  {}", r);
                    }
                }
            }
        }
        return;
    }

    // Full triage display
    if let Some(triage_data) = triage.get("triage") {
        if let Some(quick_ref) = triage_data.get("quick_ref") {
            let open = quick_ref.get("open_count").and_then(|v| v.as_i64()).unwrap_or(0);
            let actionable = quick_ref.get("actionable_count").and_then(|v| v.as_i64()).unwrap_or(0);
            let blocked = quick_ref.get("blocked_count").and_then(|v| v.as_i64()).unwrap_or(0);
            let in_progress = quick_ref.get("in_progress_count").and_then(|v| v.as_i64()).unwrap_or(0);
            
            println!("Cross-Repo Triage");
            println!("=================");
            println!("Open: {}  Actionable: {}  Blocked: {}  In Progress: {}", 
                     open, actionable, blocked, in_progress);
            println!();
            
            if let Some(top_picks) = quick_ref.get("top_picks").and_then(|v| v.as_array()) {
                println!("Top Picks:");
                for pick in top_picks.iter().take(5) {
                    let id = pick.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    let title = pick.get("title").and_then(|v| v.as_str()).unwrap_or("?");
                    let score = pick.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let unblocks = pick.get("unblocks").and_then(|v| v.as_i64()).unwrap_or(0);
                    println!("  [{:.2}] {} (unblocks {})", score, id, unblocks);
                    println!("        {}", title);
                }
            }
        }
    }
}

fn handle_search(ctx: &RuntimeContext, cmd: SearchCommand) -> Result<()> {
    use std::process::Command;

    let mut args = vec!["search".to_string(), cmd.query.clone()];
    
    // Add workspace filter if repo specified
    if let Some(ref repo) = cmd.repo {
        // Try to find the workspace path for this repo
        let workspace_path = ctx.workspace_root().join(repo);
        if workspace_path.exists() {
            args.push("--workspace".to_string());
            args.push(workspace_path.display().to_string());
        }
    }
    
    args.push("--limit".to_string());
    args.push(cmd.limit.to_string());
    
    if let Some(days) = cmd.days {
        args.push("--days".to_string());
        args.push(days.to_string());
    }
    
    if ctx.common.json {
        args.push("--json".to_string());
    }

    let output = Command::new("cass")
        .args(&args)
        .output()
        .context("running cass search - is cass installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("cass search failed: {}", stderr));
    }

    let content = String::from_utf8_lossy(&output.stdout);
    println!("{}", content);

    Ok(())
}

fn handle_memory(ctx: &RuntimeContext, command: MemoryCommand) -> Result<()> {
    use std::process::Command;

    match command {
        MemoryCommand::Add { content, project, govnr, category, tags, importance } => {
            // Determine store: explicit --govnr, explicit --project, or auto-detect from cwd
            let store_name = if govnr {
                "govnr".to_string()
            } else if let Some(ref proj) = project {
                validate_project_name(ctx, proj)?
            } else {
                // Auto-detect from current directory
                detect_current_project(ctx)?
            };
            
            // Ensure store exists (create if needed)
            ensure_memory_store(&store_name)?;
            
            let mut args = vec!["add".to_string(), content];
            
            args.push("--store".to_string());
            args.push(store_name.clone());
            
            args.push("--memory-type".to_string());
            args.push("semantic".to_string());
            
            if let Some(cat) = category {
                args.push("--category".to_string());
                args.push(cat);
            }
            
            if let Some(t) = tags {
                args.push("--tags".to_string());
                args.push(t);
            }
            
            args.push("--importance".to_string());
            args.push(importance.to_string());
            
            if ctx.common.json {
                args.push("--json".to_string());
            }

            let output = Command::new("mmry")
                .args(&args)
                .output()
                .context("running mmry add - is mmry installed?")?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            if !stdout.is_empty() {
                println!("{}", stdout);
            }
            
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("{}", stderr);
            } else if !ctx.common.json {
                println!("Memory added to project: {}", store_name);
            }
        }
        MemoryCommand::Search { query, project, govnr, all, limit } => {
            let mut args = vec![
                "search".to_string(),
                query,
                "--limit".to_string(),
                limit.to_string(),
            ];
            
            if all {
                args.push("--all-stores".to_string());
            } else if govnr {
                args.push("--store".to_string());
                args.push("govnr".to_string());
            } else if let Some(ref proj) = project {
                let store_name = validate_project_name(ctx, proj)?;
                args.push("--store".to_string());
                args.push(store_name);
            } else {
                // Auto-detect from current directory
                let store_name = detect_current_project(ctx)?;
                args.push("--store".to_string());
                args.push(store_name);
            }
            
            if ctx.common.json {
                args.push("--json".to_string());
            }

            let output = Command::new("mmry")
                .args(&args)
                .output()
                .context("running mmry search - is mmry installed?")?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            println!("{}", stdout);
            
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("{}", stderr);
            }
        }
        MemoryCommand::Projects => {
            list_memory_projects(ctx)?;
        }
    }

    Ok(())
}

/// Detect the current project based on working directory
fn detect_current_project(ctx: &RuntimeContext) -> Result<String> {
    let cwd = env::current_dir()?;
    let workspace = ctx.workspace_root();
    
    // Check if cwd is within the workspace
    if let Ok(relative) = cwd.strip_prefix(workspace) {
        // Get the first component (repo name)
        if let Some(first) = relative.components().next() {
            if let Some(name) = first.as_os_str().to_str() {
                // Check if this is a valid repo in the catalog
                let catalog_path = ctx.catalog_path();
                if catalog_path.exists() {
                    let content = fs::read_to_string(&catalog_path)?;
                    let catalog: Catalog = serde_json::from_str(&content)?;
                    
                    if catalog.repos.contains_key(name) {
                        info!("Auto-detected project: {}", name);
                        return Ok(name.to_string());
                    }
                }
            }
        }
    }
    
    // If at workspace root or outside workspace, use govnr
    info!("At workspace root or outside workspace, using govnr store");
    Ok("govnr".to_string())
}

/// Validate project name against catalog or allow special names
fn validate_project_name(ctx: &RuntimeContext, project: &str) -> Result<String> {
    // Special names always allowed
    if project == "govnr" || project == "default" {
        return Ok(project.to_string());
    }
    
    // Check if project exists in catalog
    let catalog_path = ctx.catalog_path();
    if catalog_path.exists() {
        let content = fs::read_to_string(&catalog_path)?;
        let catalog: Catalog = serde_json::from_str(&content)?;
        
        if catalog.repos.contains_key(project) {
            return Ok(project.to_string());
        }
        
        // Suggest similar names if not found
        let similar: Vec<&String> = catalog.repos.keys()
            .filter(|k| k.contains(project) || project.contains(k.as_str()))
            .take(3)
            .collect();
        
        if !similar.is_empty() {
            return Err(anyhow!(
                "Unknown project '{}'. Did you mean: {}?\nRun 'byt memory projects' to list available projects.",
                project,
                similar.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    
    Err(anyhow!(
        "Unknown project '{}'. Run 'byt memory projects' to list available projects, or use 'govnr' for cross-repo memories.",
        project
    ))
}

/// Ensure a memory store exists, creating it if needed
fn ensure_memory_store(store_name: &str) -> Result<()> {
    use std::process::Command;
    
    // Check if store exists
    let output = Command::new("mmry")
        .args(["stores", "list"])
        .output()
        .context("checking mmry stores")?;
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    
    // If store already exists, we're done
    if stdout.contains(&format!("{} ", store_name)) || stdout.contains(&format!("{} (default)", store_name)) {
        return Ok(());
    }
    
    // Create the store
    info!("Creating memory store: {}", store_name);
    let output = Command::new("mmry")
        .args(["stores", "create", store_name])
        .output()
        .context("creating mmry store")?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Ignore "already exists" errors
        if !stderr.contains("already exists") {
            return Err(anyhow!("Failed to create store '{}': {}", store_name, stderr));
        }
    }
    
    Ok(())
}

/// List available projects for memory storage
fn list_memory_projects(ctx: &RuntimeContext) -> Result<()> {
    use std::process::Command;
    
    // Get existing stores
    let output = Command::new("mmry")
        .args(["stores", "list"])
        .output()
        .context("listing mmry stores")?;
    
    let stores_output = String::from_utf8_lossy(&output.stdout);
    
    // Parse existing stores
    let existing_stores: Vec<&str> = stores_output
        .lines()
        .filter(|l| l.starts_with("  "))
        .filter_map(|l| l.trim().split_whitespace().next())
        .collect();
    
    // Get catalog repos
    let catalog_path = ctx.catalog_path();
    let catalog_repos: Vec<String> = if catalog_path.exists() {
        let content = fs::read_to_string(&catalog_path)?;
        let catalog: Catalog = serde_json::from_str(&content)?;
        catalog.repos.keys().cloned().collect()
    } else {
        Vec::new()
    };
    
    if ctx.common.json {
        #[derive(Serialize)]
        struct ProjectsOutput {
            special: Vec<String>,
            repos: Vec<String>,
            existing_stores: Vec<String>,
        }
        
        let output = ProjectsOutput {
            special: vec!["govnr".to_string()],
            repos: catalog_repos,
            existing_stores: existing_stores.iter().map(|s| s.to_string()).collect(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Available Projects for Memory Storage");
        println!("======================================");
        println!();
        println!("Special:");
        println!("  govnr     - Cross-repo governance memories (default)");
        println!();
        println!("Existing Stores:");
        for store in &existing_stores {
            let marker = if *store == "govnr" { " (govnr)" } else { "" };
            println!("  {}{}", store, marker);
        }
        println!();
        println!("Catalog Repos ({} available):", catalog_repos.len());
        for repo in catalog_repos.iter().take(20) {
            let has_store = existing_stores.contains(&repo.as_str());
            let marker = if has_store { " [has store]" } else { "" };
            println!("  {}{}", repo, marker);
        }
        if catalog_repos.len() > 20 {
            println!("  ... and {} more (run 'byt catalog list' for full list)", catalog_repos.len() - 20);
        }
        println!();
        println!("Usage:");
        println!("  byt memory add \"content\" --project govnr     # Cross-repo memory");
        println!("  byt memory add \"content\" --project omni      # Repo-specific memory");
        println!("  byt memory search \"query\" --project omni     # Search repo memories");
        println!("  byt memory search \"query\" --all              # Search all projects");
    }
    
    Ok(())
}

// ============================================================================
// Sync Command Handlers
// ============================================================================

fn handle_sync(ctx: &RuntimeContext, command: SyncCommand) -> Result<()> {
    match command {
        SyncCommand::Push { stores } => sync_push(ctx, stores),
        SyncCommand::Pull { stores } => sync_pull(ctx, stores),
        SyncCommand::Status => sync_status(ctx),
    }
}

/// Get list of stores to sync (govnr + repos that exist locally)
fn get_syncable_stores(ctx: &RuntimeContext, explicit_stores: Vec<String>) -> Result<Vec<String>> {
    if !explicit_stores.is_empty() {
        return Ok(explicit_stores);
    }
    
    let mut stores = vec!["govnr".to_string()];
    
    // Get repos from catalog that exist locally
    let catalog_path = ctx.catalog_path();
    if catalog_path.exists() {
        let content = fs::read_to_string(&catalog_path)?;
        let catalog: Catalog = serde_json::from_str(&content)?;
        
        // Get existing mmry stores
        let existing = get_existing_stores()?;
        
        // Add repos that have stores (skip govnr since it's already added)
        for repo_name in catalog.repos.keys() {
            if repo_name != "govnr" && existing.contains(repo_name) {
                stores.push(repo_name.clone());
            }
        }
    }
    
    Ok(stores)
}

/// Get list of existing mmry stores
fn get_existing_stores() -> Result<Vec<String>> {
    use std::process::Command;
    
    let output = Command::new("mmry")
        .args(["stores", "list"])
        .output()
        .context("listing mmry stores")?;
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    
    let stores: Vec<String> = stdout
        .lines()
        .filter(|l| l.starts_with("  "))
        .filter_map(|l| l.trim().split_whitespace().next())
        .map(|s| s.to_string())
        .collect();
    
    Ok(stores)
}

/// Export memories to .sync/memories/ for git-based sync
fn sync_push(ctx: &RuntimeContext, explicit_stores: Vec<String>) -> Result<()> {
    use std::process::Command;
    
    let stores = get_syncable_stores(ctx, explicit_stores)?;
    let sync_dir = ctx.workspace_root().join(".sync/memories");
    
    fs::create_dir_all(&sync_dir)?;
    
    if ctx.common.dry_run {
        println!("Would export {} stores to {}", stores.len(), sync_dir.display());
        for store in &stores {
            println!("  - {}", store);
        }
        return Ok(());
    }
    
    let mut exported = 0;
    let mut total_memories = 0;
    
    for store in &stores {
        let output_file = sync_dir.join(format!("{}.json", store));
        
        let output = Command::new("mmry")
            .args(["export", "--store", store, "-o", &output_file.display().to_string()])
            .output()
            .with_context(|| format!("exporting store '{}'", store))?;
        
        if output.status.success() {
            // Count memories in export
            if let Ok(content) = fs::read_to_string(&output_file) {
                if let Ok(export) = serde_json::from_str::<serde_json::Value>(&content) {
                    let count = export.get("memory_count").and_then(|v| v.as_i64()).unwrap_or(0);
                    total_memories += count;
                    if !ctx.common.quiet {
                        println!("  {} ({} memories)", store, count);
                    }
                }
            }
            exported += 1;
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("  {} - FAILED: {}", store, stderr.trim());
        }
    }
    
    if ctx.common.json {
        #[derive(Serialize)]
        struct PushResult {
            stores_exported: usize,
            total_memories: i64,
            sync_dir: String,
        }
        println!("{}", serde_json::to_string_pretty(&PushResult {
            stores_exported: exported,
            total_memories,
            sync_dir: sync_dir.display().to_string(),
        })?);
    } else {
        println!();
        println!("Exported {} stores ({} memories) to {}", exported, total_memories, sync_dir.display());
        println!("Run 'git add .sync && git commit' to sync");
    }
    
    Ok(())
}

/// Import memories from .sync/memories/ after git pull
fn sync_pull(ctx: &RuntimeContext, explicit_stores: Vec<String>) -> Result<()> {
    use std::process::Command;
    
    let sync_dir = ctx.workspace_root().join(".sync/memories");
    
    if !sync_dir.exists() {
        return Err(anyhow!("No .sync/memories directory found. Run 'byt sync push' first or 'git pull'."));
    }
    
    // Get stores to import
    let stores = if explicit_stores.is_empty() {
        // Auto-detect from files in sync dir that match local repos
        let catalog_path = ctx.catalog_path();
        let local_repos: Vec<String> = if catalog_path.exists() {
            let content = fs::read_to_string(&catalog_path)?;
            let catalog: Catalog = serde_json::from_str(&content)?;
            catalog.repos.keys().cloned().collect()
        } else {
            Vec::new()
        };
        
        let mut stores = Vec::new();
        for entry in fs::read_dir(&sync_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    // Always sync govnr, or sync if repo exists locally
                    if stem == "govnr" || local_repos.contains(&stem.to_string()) {
                        stores.push(stem.to_string());
                    }
                }
            }
        }
        stores
    } else {
        explicit_stores
    };
    
    if stores.is_empty() {
        println!("No stores to import (no matching repos found locally)");
        return Ok(());
    }
    
    if ctx.common.dry_run {
        println!("Would import {} stores from {}", stores.len(), sync_dir.display());
        for store in &stores {
            println!("  - {}", store);
        }
        return Ok(());
    }
    
    let mut imported = 0;
    let mut total_memories = 0;
    let mut skipped = 0;
    
    for store in &stores {
        let input_file = sync_dir.join(format!("{}.json", store));
        
        if !input_file.exists() {
            if !ctx.common.quiet {
                println!("  {} - skipped (no export file)", store);
            }
            continue;
        }
        
        // Ensure store exists
        ensure_memory_store(store)?;
        
        // Read export file
        let content = fs::read_to_string(&input_file)?;
        let export: serde_json::Value = serde_json::from_str(&content)?;
        
        let memories = export.get("memories").and_then(|v| v.as_array());
        if memories.is_none() {
            continue;
        }
        
        let memories = memories.unwrap();
        let mut store_imported = 0;
        let mut store_skipped = 0;
        
        for memory in memories {
            // Try to add the memory via stdin (mmry will handle deduplication via content hash)
            let memory_json = serde_json::to_string(memory)?;
            
            let mut child = Command::new("mmry")
                .args(["add", "-", "--store", store])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .context("spawning mmry add")?;
            
            // Write JSON to stdin and close it
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                stdin.write_all(memory_json.as_bytes())?;
                // stdin is dropped here, closing the pipe
            }
            
            // Wait for the command to complete
            let output = child.wait_with_output()?;
            
            if output.status.success() {
                store_imported += 1;
            } else {
                // Likely duplicate, skip silently
                store_skipped += 1;
            }
        }
        
        if !ctx.common.quiet {
            println!("  {} ({} imported, {} skipped)", store, store_imported, store_skipped);
        }
        
        imported += store_imported;
        skipped += store_skipped;
        total_memories += memories.len();
    }
    
    if ctx.common.json {
        #[derive(Serialize)]
        struct PullResult {
            memories_imported: usize,
            memories_skipped: usize,
            total_in_sync: usize,
        }
        println!("{}", serde_json::to_string_pretty(&PullResult {
            memories_imported: imported,
            memories_skipped: skipped,
            total_in_sync: total_memories,
        })?);
    } else {
        println!();
        println!("Imported {} memories ({} skipped as duplicates)", imported, skipped);
    }
    
    Ok(())
}

/// Show sync status
fn sync_status(ctx: &RuntimeContext) -> Result<()> {
    let sync_dir = ctx.workspace_root().join(".sync/memories");
    let existing_stores = get_existing_stores()?;
    
    // Get catalog repos
    let catalog_path = ctx.catalog_path();
    let catalog_repos: Vec<String> = if catalog_path.exists() {
        let content = fs::read_to_string(&catalog_path)?;
        let catalog: Catalog = serde_json::from_str(&content)?;
        catalog.repos.keys().cloned().collect()
    } else {
        Vec::new()
    };
    
    if ctx.common.json {
        #[derive(Serialize)]
        struct SyncStatus {
            sync_dir_exists: bool,
            sync_files: Vec<String>,
            local_stores: Vec<String>,
            syncable_stores: Vec<String>,
        }
        
        let sync_files: Vec<String> = if sync_dir.exists() {
            fs::read_dir(&sync_dir)?
                .filter_map(|e| e.ok())
                .filter_map(|e| e.path().file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()))
                .collect()
        } else {
            Vec::new()
        };
        
        let syncable: Vec<String> = existing_stores.iter()
            .filter(|s| *s == "govnr" || catalog_repos.contains(s))
            .cloned()
            .collect();
        
        println!("{}", serde_json::to_string_pretty(&SyncStatus {
            sync_dir_exists: sync_dir.exists(),
            sync_files,
            local_stores: existing_stores,
            syncable_stores: syncable,
        })?);
    } else {
        println!("Memory Sync Status");
        println!("==================");
        println!();
        
        println!("Sync directory: {}", sync_dir.display());
        println!("  Exists: {}", if sync_dir.exists() { "yes" } else { "no" });
        
        if sync_dir.exists() {
            println!("  Files:");
            for entry in fs::read_dir(&sync_dir)? {
                let entry = entry?;
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    println!("    {} ({} bytes)", name, size);
                }
            }
        }
        
        println!();
        println!("Local stores:");
        for store in &existing_stores {
            let syncable = *store == "govnr" || catalog_repos.contains(store);
            let marker = if syncable { "[syncable]" } else { "[local-only]" };
            println!("  {} {}", store, marker);
        }
        
        println!();
        println!("Commands:");
        println!("  byt sync push    # Export memories to .sync/");
        println!("  byt sync pull    # Import memories from .sync/");
    }
    
    Ok(())
}

fn handle_init(ctx: &RuntimeContext, cmd: InitCommand) -> Result<()> {
    if ctx.paths.config_file.exists() && !cmd.force {
        return Err(anyhow!(
            "Config already exists at {} (use --force to overwrite)",
            ctx.paths.config_file.display()
        ));
    }

    if ctx.common.dry_run {
        info!(
            "dry-run: would write default config to {}",
            ctx.paths.config_file.display()
        );
        return Ok(());
    }

    write_default_config(&ctx.paths.config_file)
}

fn handle_config(ctx: &RuntimeContext, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Show => {
            if ctx.common.json {
                println!("{}", serde_json::to_string_pretty(&ctx.config)?);
            } else {
                println!("{:#?}", ctx.config);
            }
            Ok(())
        }
        ConfigCommand::Path => {
            println!("{}", ctx.paths.config_file.display());
            Ok(())
        }
    }
}

fn handle_secrets(ctx: &RuntimeContext, command: SecretsCommand) -> Result<()> {
    use std::process::Command;
    
    // Check if gh is available
    let gh_check = Command::new("gh")
        .arg("--version")
        .output();
    
    if gh_check.is_err() {
        return Err(anyhow!("GitHub CLI (gh) not found. Install it from https://cli.github.com"));
    }
    
    let release_cfg = &ctx.config.release;
    
    // Normalize repo name to full path using configured org
    let normalize_repo = |repo: &str| -> Result<String> {
        if repo.contains('/') {
            Ok(repo.to_string())
        } else if !release_cfg.github_org.is_empty() {
            Ok(format!("{}/{}", release_cfg.github_org, repo))
        } else {
            Err(anyhow!(
                "No github_org configured. Either use full repo path (org/repo) or set github_org in ~/.config/byt/config.toml"
            ))
        }
    };
    
    match command {
        SecretsCommand::Setup { repo, skip_aur } => {
            let repo_full = normalize_repo(&repo)?;
            
            println!("Setting up release secrets for {}", repo_full);
            println!();
            
            // 1. TAP_GITHUB_TOKEN
            let token_name = &release_cfg.tap_token_name;
            println!("=== {} ===", token_name);
            
            // Show which repos need access
            let mut needs_access = Vec::new();
            if !release_cfg.homebrew_tap.is_empty() {
                needs_access.push(release_cfg.homebrew_tap.as_str());
            }
            if !release_cfg.scoop_bucket.is_empty() {
                needs_access.push(release_cfg.scoop_bucket.as_str());
            }
            if needs_access.is_empty() {
                println!("This token needs 'repo' scope for your homebrew-tap and scoop-bucket repos");
            } else {
                println!("This token needs 'repo' scope for: {}", needs_access.join(", "));
            }
            println!();
            
            // Check if already set
            let check = Command::new("gh")
                .args(["secret", "list", "--repo", &repo_full])
                .output()
                .context("checking existing secrets")?;
            
            let existing = String::from_utf8_lossy(&check.stdout);
            let has_tap_token = existing.lines().any(|l| l.starts_with(token_name));
            
            if has_tap_token {
                println!("{} is already set. Skipping (use 'byt secrets set' to update)", token_name);
            } else {
                println!("Enter your GitHub PAT (or press Enter to skip):");
                print!("> ");
                io::stdout().flush()?;
                
                let mut token = String::new();
                io::stdin().read_line(&mut token)?;
                let token = token.trim();
                
                if !token.is_empty() {
                    let mut child = Command::new("gh")
                        .args(["secret", "set", token_name, "--repo", &repo_full])
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                        .context("setting secret")?;
                    
                    if let Some(mut stdin) = child.stdin.take() {
                        stdin.write_all(token.as_bytes())?;
                    }
                    
                    let status = child.wait()?;
                    if status.success() {
                        println!("{} set successfully", token_name);
                    } else {
                        eprintln!("Failed to set {}", token_name);
                    }
                }
            }
            
            // 2. AUR secrets (if not skipped)
            if !skip_aur {
                println!();
                println!("=== AUR_SSH_PRIVATE_KEY ===");
                
                let has_aur_key = existing.lines().any(|l| l.starts_with("AUR_SSH_PRIVATE_KEY"));
                
                if has_aur_key {
                    println!("AUR_SSH_PRIVATE_KEY is already set. Skipping.");
                } else if let Some(ref key_path) = ctx.config.release.aur_ssh_key_path {
                    let expanded = shellexpand::tilde(key_path);
                    let key_file = Path::new(expanded.as_ref());
                    
                    if key_file.exists() {
                        println!("Reading from: {}", key_file.display());
                        let key_content = fs::read_to_string(key_file)
                            .context("reading AUR SSH key")?;
                        
                        let mut child = Command::new("gh")
                            .args(["secret", "set", "AUR_SSH_PRIVATE_KEY", "--repo", &repo_full])
                            .stdin(std::process::Stdio::piped())
                            .spawn()
                            .context("setting secret")?;
                        
                        if let Some(mut stdin) = child.stdin.take() {
                            stdin.write_all(key_content.as_bytes())?;
                        }
                        
                        let status = child.wait()?;
                        if status.success() {
                            println!("AUR_SSH_PRIVATE_KEY set successfully");
                        } else {
                            eprintln!("Failed to set AUR_SSH_PRIVATE_KEY");
                        }
                    } else {
                        eprintln!("AUR SSH key not found at: {}", key_file.display());
                        eprintln!("Configure aur_ssh_key_path in ~/.config/byt/config.toml");
                    }
                } else {
                    eprintln!("No aur_ssh_key_path configured in ~/.config/byt/config.toml");
                    eprintln!("Skipping AUR_SSH_PRIVATE_KEY");
                }
                
                println!();
                println!("=== AUR_EMAIL ===");
                
                let has_aur_email = existing.lines().any(|l| l.starts_with("AUR_EMAIL"));
                
                if has_aur_email {
                    println!("AUR_EMAIL is already set. Skipping.");
                } else if let Some(ref email) = ctx.config.release.aur_email {
                    let mut child = Command::new("gh")
                        .args(["secret", "set", "AUR_EMAIL", "--repo", &repo_full])
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                        .context("setting secret")?;
                    
                    if let Some(mut stdin) = child.stdin.take() {
                        stdin.write_all(email.as_bytes())?;
                    }
                    
                    let status = child.wait()?;
                    if status.success() {
                        println!("AUR_EMAIL set to: {}", email);
                    } else {
                        eprintln!("Failed to set AUR_EMAIL");
                    }
                } else {
                    eprintln!("No aur_email configured in ~/.config/byt/config.toml");
                    eprintln!("Skipping AUR_EMAIL");
                }
            }
            
            println!();
            println!("Done! Run 'byt secrets list {}' to verify.", repo);
            Ok(())
        }
        
        SecretsCommand::List { repo } => {
            let repo_full = normalize_repo(&repo)?;
            
            let output = Command::new("gh")
                .args(["secret", "list", "--repo", &repo_full])
                .output()
                .context("listing secrets")?;
            
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow!("Failed to list secrets: {}", stderr));
            }
            
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.is_empty() {
                println!("No secrets found for {}", repo_full);
            } else {
                println!("Secrets for {}:", repo_full);
                println!();
                print!("{}", stdout);
            }
            Ok(())
        }
        
        SecretsCommand::Set { repo, name, value, from_file } => {
            let repo_full = normalize_repo(&repo)?;
            
            let secret_value = if let Some(v) = value {
                v
            } else if let Some(path) = from_file {
                fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?
            } else {
                // Prompt for value
                println!("Enter value for {} (Ctrl+D when done):", name);
                let mut val = String::new();
                io::stdin().read_line(&mut val)?;
                val.trim().to_string()
            };
            
            let mut child = Command::new("gh")
                .args(["secret", "set", &name, "--repo", &repo_full])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .context("setting secret")?;
            
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(secret_value.as_bytes())?;
            }
            
            let status = child.wait()?;
            if status.success() {
                println!("Secret {} set for {}", name, repo_full);
            } else {
                return Err(anyhow!("Failed to set secret {}", name));
            }
            Ok(())
        }
    }
}

fn handle_new(ctx: &RuntimeContext, cmd: NewCommand) -> Result<()> {
    use std::process::Command;
    
    let templates_cfg = &ctx.config.templates;
    let release_cfg = &ctx.config.release;
    
    // Determine output directory
    let output_dir = cmd.output.unwrap_or_else(|| PathBuf::from("."));
    let project_dir = output_dir.join(&cmd.name);
    
    if project_dir.exists() {
        return Err(anyhow!("Directory '{}' already exists", project_dir.display()));
    }
    
    println!("Creating new {} project: {}", cmd.template, cmd.name);
    
    // Clone template from GitHub
    let template_url = format!(
        "https://github.com/{}/archive/refs/heads/{}.zip",
        templates_cfg.repo, templates_cfg.branch
    );
    
    // Create temp dir for download
    let temp_dir = std::env::temp_dir().join(format!("byt-template-{}", std::process::id()));
    fs::create_dir_all(&temp_dir)?;
    
    // Download and extract template
    println!("Fetching template from {}...", templates_cfg.repo);
    
    let zip_path = temp_dir.join("template.zip");
    let output = Command::new("curl")
        .args(["-sL", "-o", zip_path.to_str().unwrap(), &template_url])
        .output()
        .context("downloading template")?;
    
    if !output.status.success() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(anyhow!("Failed to download template"));
    }
    
    // Extract zip
    let extract_dir = temp_dir.join("extracted");
    let output = Command::new("unzip")
        .args(["-q", zip_path.to_str().unwrap(), "-d", extract_dir.to_str().unwrap()])
        .output()
        .context("extracting template")?;
    
    if !output.status.success() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(anyhow!("Failed to extract template"));
    }
    
    // Find the templates root directory (repo-branch/)
    let repo_name = templates_cfg.repo.split('/').last().unwrap_or("templates");
    let templates_root = extract_dir.join(format!("{}-{}", repo_name, templates_cfg.branch));
    let template_src = templates_root.join(&cmd.template);
    
    if !template_src.exists() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(anyhow!(
            "Template '{}' not found. Available: rust-cli, rust-workspace, python-cli, go-cli",
            cmd.template
        ));
    }
    
    // Check for template manifest (template.toml)
    let manifest_path = template_src.join("template.toml");
    let manifest: Option<TemplateManifest> = if manifest_path.exists() {
        let content = fs::read_to_string(&manifest_path)
            .context("reading template.toml")?;
        Some(toml::from_str(&content).context("parsing template.toml")?)
    } else {
        None
    };
    
    // Copy template to project directory
    println!("Creating project structure...");
    copy_dir_recursive(&template_src, &project_dir)?;
    
    // Remove template.toml from the project (it's only for byt)
    let project_manifest = project_dir.join("template.toml");
    if project_manifest.exists() {
        fs::remove_file(&project_manifest)?;
    }
    
    // Apply template composition if defined
    if let Some(ref manifest) = manifest {
        for comp in &manifest.compose {
            let source_template = templates_root.join(&comp.source);
            if !source_template.exists() {
                eprintln!("Warning: Composed template '{}' not found, skipping", comp.source);
                continue;
            }
            
            let target_dir = project_dir.join(&comp.target);
            println!("  Composing {} -> {}", comp.source, comp.target);
            
            // Remove target directory if it exists (will be replaced by composed template)
            if target_dir.exists() {
                fs::remove_dir_all(&target_dir)?;
            }
            
            // Copy composed template, excluding specified files
            copy_dir_filtered(&source_template, &target_dir, &comp.exclude)?;
            
            // Remove template.toml from composed template too
            let composed_manifest = target_dir.join("template.toml");
            if composed_manifest.exists() {
                fs::remove_file(&composed_manifest)?;
            }
        }
    }
    
    // Replace template variables
    println!("Configuring project...");
    replace_in_files(&project_dir, "{{project_name}}", &cmd.name)?;
    replace_in_files(&project_dir, "your-binary-name", &cmd.name)?;
    
    // Rename directories/files if needed (e.g., python_cli -> project_name)
    rename_template_dirs(&project_dir, &cmd.template, &cmd.name)?;
    
    // Clean up temp
    let _ = fs::remove_dir_all(&temp_dir);
    
    // Initialize git
    println!("Initializing git...");
    let _ = Command::new("git")
        .args(["init"])
        .current_dir(&project_dir)
        .output();
    
    // Initialize beads
    println!("Initializing beads...");
    let _ = Command::new("bd")
        .args(["init"])
        .current_dir(&project_dir)
        .output();
    
    // Create GitHub repo if requested
    if cmd.github {
        println!("Creating GitHub repository...");
        
        let mut gh_args = vec!["repo", "create"];
        
        let repo_name = if !release_cfg.github_org.is_empty() {
            format!("{}/{}", release_cfg.github_org, cmd.name)
        } else {
            cmd.name.clone()
        };
        gh_args.push(&repo_name);
        
        if cmd.private {
            gh_args.push("--private");
        } else {
            gh_args.push("--public");
        }
        
        gh_args.push("--source");
        gh_args.push(project_dir.to_str().unwrap());
        
        if let Some(ref desc) = cmd.description {
            gh_args.push("--description");
            gh_args.push(desc);
        }
        
        gh_args.push("--push");
        
        let output = Command::new("gh")
            .args(&gh_args)
            .output()
            .context("creating GitHub repo")?;
        
        if output.status.success() {
            println!("GitHub repository created: {}", repo_name);
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Warning: Failed to create GitHub repo: {}", stderr.trim());
        }
    }
    
    println!();
    println!("Project created at: {}", project_dir.display());
    println!();
    println!("Next steps:");
    println!("  cd {}", cmd.name);
    println!("  just");
    if !cmd.github {
        println!("  # To create GitHub repo: byt new {} --github", cmd.name);
    }
    
    Ok(())
}

/// Recursively copy a directory
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    
    Ok(())
}

/// Recursively copy a directory, excluding specified paths
fn copy_dir_filtered(src: &Path, dst: &Path, exclude: &[String]) -> Result<()> {
    fs::create_dir_all(dst)?;
    
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();
        
        // Check if this file/dir should be excluded
        let should_exclude = exclude.iter().any(|pattern| {
            // Simple matching: exact name or glob pattern
            if pattern.contains('*') {
                // Basic glob: *.ext or prefix*
                if let Some(suffix) = pattern.strip_prefix('*') {
                    file_name.ends_with(suffix)
                } else if let Some(prefix) = pattern.strip_suffix('*') {
                    file_name.starts_with(prefix)
                } else {
                    false
                }
            } else {
                file_name == *pattern
            }
        });
        
        if should_exclude {
            continue;
        }
        
        let dst_path = dst.join(entry.file_name());
        
        if src_path.is_dir() {
            copy_dir_filtered(&src_path, &dst_path, exclude)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    
    Ok(())
}

/// Replace a string in all files in a directory recursively
fn replace_in_files(dir: &Path, from: &str, to: &str) -> Result<()> {
    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry?;
        let path = entry.path();
        
        if path.is_file() {
            // Skip binary files
            if let Some(ext) = path.extension() {
                let ext = ext.to_string_lossy();
                if ["png", "jpg", "jpeg", "gif", "ico", "woff", "woff2", "ttf", "lock"].contains(&ext.as_ref()) {
                    continue;
                }
            }
            
            if let Ok(content) = fs::read_to_string(path) {
                if content.contains(from) {
                    let new_content = content.replace(from, to);
                    fs::write(path, new_content)?;
                }
            }
        }
    }
    
    Ok(())
}

/// Rename template-specific directories
fn rename_template_dirs(project_dir: &Path, template: &str, project_name: &str) -> Result<()> {
    // Python: rename python_cli to project_name (with underscores)
    if template == "python-cli" {
        let old_dir = project_dir.join("python_cli");
        if old_dir.exists() {
            let new_name = project_name.replace('-', "_");
            let new_dir = project_dir.join(&new_name);
            fs::rename(&old_dir, &new_dir)?;
            
            // Also update imports in files
            replace_in_files(project_dir, "python_cli", &new_name)?;
        }
    }
    
    // Rust workspace: rename crate directories
    if template == "rust-workspace" {
        let crates_dir = project_dir.join("crates");
        if crates_dir.exists() {
            for entry in fs::read_dir(&crates_dir)? {
                let entry = entry?;
                let old_name = entry.file_name().to_string_lossy().to_string();
                if old_name.starts_with("rust-") {
                    let new_name = old_name.replace("rust-", &format!("{}-", project_name));
                    let new_path = crates_dir.join(&new_name);
                    fs::rename(entry.path(), &new_path)?;
                }
            }
            // Update Cargo.toml references
            replace_in_files(project_dir, "rust-cli", &format!("{}-cli", project_name))?;
            replace_in_files(project_dir, "rust-core", &format!("{}-core", project_name))?;
            replace_in_files(project_dir, "rust-api", &format!("{}-api", project_name))?;
            replace_in_files(project_dir, "rust-mcp", &format!("{}-mcp", project_name))?;
            replace_in_files(project_dir, "rust-tui", &format!("{}-tui", project_name))?;
        }
    }
    
    Ok(())
}

// ============================================================================
// Schema Management
// ============================================================================

#[derive(Debug)]
struct SchemaInfo {
    repo: String,
    source_path: PathBuf,
    dest_path: PathBuf,
    needs_update: bool,
}

fn handle_schema(ctx: &RuntimeContext, command: SchemaCommand) -> Result<()> {
    match command {
        SchemaCommand::Check => schema_check(ctx),
        SchemaCommand::Sync { push, repos } => schema_sync(ctx, push, repos),
        SchemaCommand::List => schema_list(ctx),
    }
}

fn find_schemas(ctx: &RuntimeContext) -> Result<Vec<SchemaInfo>> {
    use glob::glob;
    
    let workspace = ctx.workspace_root();
    let schema_cfg = &ctx.config.schemas;
    
    // Get schemas repo path
    let schemas_repo = if PathBuf::from(&schema_cfg.repo_path).is_absolute() {
        PathBuf::from(&schema_cfg.repo_path)
    } else {
        workspace.join(&schema_cfg.repo_path)
    };
    
    // Load catalog to get repo list
    let catalog_path = ctx.catalog_path();
    if !catalog_path.exists() {
        return Err(anyhow!("Catalog not found at {}. Run 'byt catalog refresh' first.", catalog_path.display()));
    }
    
    let content = fs::read_to_string(&catalog_path)?;
    let catalog: Catalog = serde_json::from_str(&content)?;
    
    let mut schemas = Vec::new();
    
    for (repo_name, repo_info) in &catalog.repos {
        let repo_path = workspace.join(&repo_info.path);
        
        // Check each pattern for schema files
        for pattern in &schema_cfg.patterns {
            let full_pattern = repo_path.join(pattern);
            if let Some(pattern_str) = full_pattern.to_str() {
                for entry in glob(pattern_str).unwrap_or_else(|_| glob("").unwrap()) {
                    if let Ok(source_path) = entry {
                        // Determine destination path in schemas repo
                        let file_name = source_path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("schema.json");
                        
                        // Use repo name as directory, preserve original filename
                        // Only rename config.schema.json -> {repo}.config.schema.json for clarity
                        let dest_name = if file_name == "config.schema.json" {
                            format!("{}.config.schema.json", repo_name)
                        } else {
                            // Keep original filename for all other schemas
                            file_name.to_string()
                        };
                        
                        let dest_path = schemas_repo.join(repo_name).join(&dest_name);
                        
                        // Check if needs update
                        let needs_update = if dest_path.exists() {
                            // Compare file contents
                            let source_content = fs::read_to_string(&source_path).unwrap_or_default();
                            let dest_content = fs::read_to_string(&dest_path).unwrap_or_default();
                            source_content != dest_content
                        } else {
                            true
                        };
                        
                        schemas.push(SchemaInfo {
                            repo: repo_name.clone(),
                            source_path,
                            dest_path,
                            needs_update,
                        });
                    }
                }
            }
        }
    }
    
    Ok(schemas)
}

fn schema_check(ctx: &RuntimeContext) -> Result<()> {
    let schemas = find_schemas(ctx)?;
    
    let needs_update: Vec<_> = schemas.iter().filter(|s| s.needs_update).collect();
    
    if needs_update.is_empty() {
        println!("All schemas are up to date.");
        return Ok(());
    }
    
    println!("Schemas needing update:\n");
    for schema in &needs_update {
        let status = if schema.dest_path.exists() { "modified" } else { "new" };
        println!("  [{}] {} -> {}", 
            status,
            schema.source_path.display(),
            schema.dest_path.display()
        );
    }
    
    println!("\nRun 'byt schema sync' to update, or 'byt schema sync --push' to update and push.");
    
    Ok(())
}

fn schema_sync(ctx: &RuntimeContext, push: bool, filter_repos: Vec<String>) -> Result<()> {
    use std::process::Command;
    
    let schemas = find_schemas(ctx)?;
    let schema_cfg = &ctx.config.schemas;
    
    // Filter schemas
    let schemas: Vec<_> = if filter_repos.is_empty() {
        schemas.into_iter().filter(|s| s.needs_update).collect()
    } else {
        schemas.into_iter()
            .filter(|s| s.needs_update && filter_repos.contains(&s.repo))
            .collect()
    };
    
    if schemas.is_empty() {
        println!("No schemas to sync.");
        return Ok(());
    }
    
    // Get schemas repo path
    let workspace = ctx.workspace_root();
    let schemas_repo = if PathBuf::from(&schema_cfg.repo_path).is_absolute() {
        PathBuf::from(&schema_cfg.repo_path)
    } else {
        workspace.join(&schema_cfg.repo_path)
    };
    
    if !schemas_repo.exists() {
        return Err(anyhow!(
            "Schemas repo not found at {}. Clone it first or update schemas.repo_path in config.",
            schemas_repo.display()
        ));
    }
    
    // Copy each schema
    for schema in &schemas {
        // Create destination directory
        if let Some(parent) = schema.dest_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        // Copy file
        fs::copy(&schema.source_path, &schema.dest_path)?;
        
        let status = if schema.dest_path.exists() { "updated" } else { "added" };
        println!("[{}] {} -> {}", status, schema.repo, schema.dest_path.display());
    }
    
    if push {
        println!("\nCommitting and pushing changes...");
        
        // Git add
        let output = Command::new("git")
            .args(["add", "."])
            .current_dir(&schemas_repo)
            .output()
            .context("running git add")?;
        
        if !output.status.success() {
            return Err(anyhow!("git add failed: {}", String::from_utf8_lossy(&output.stderr)));
        }
        
        // Check if there are changes to commit
        let output = Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(&schemas_repo)
            .output()?;
        
        if output.status.success() {
            println!("No changes to commit.");
            return Ok(());
        }
        
        // Generate commit message
        let repos: std::collections::HashSet<_> = schemas.iter().map(|s| s.repo.as_str()).collect();
        let commit_msg = if repos.len() == 1 {
            format!("feat: update {} schema", repos.iter().next().unwrap())
        } else {
            let repo_list: Vec<_> = repos.into_iter().collect();
            format!("feat: update schemas for {}", repo_list.join(", "))
        };
        
        // Git commit
        let output = Command::new("git")
            .args(["commit", "-m", &commit_msg])
            .current_dir(&schemas_repo)
            .output()
            .context("running git commit")?;
        
        if !output.status.success() {
            return Err(anyhow!("git commit failed: {}", String::from_utf8_lossy(&output.stderr)));
        }
        
        // Git push
        let output = Command::new("git")
            .args(["push"])
            .current_dir(&schemas_repo)
            .output()
            .context("running git push")?;
        
        if !output.status.success() {
            return Err(anyhow!("git push failed: {}", String::from_utf8_lossy(&output.stderr)));
        }
        
        println!("Changes committed and pushed.");
    } else {
        println!("\nSchemas copied. Run 'byt schema sync --push' to commit and push.");
    }
    
    Ok(())
}

fn schema_list(ctx: &RuntimeContext) -> Result<()> {
    let schemas = find_schemas(ctx)?;
    
    if schemas.is_empty() {
        println!("No schemas found matching configured patterns.");
        println!("Patterns: {:?}", ctx.config.schemas.patterns);
        return Ok(());
    }
    
    println!("Schemas in workspace:\n");
    for schema in &schemas {
        let status = if schema.needs_update {
            if schema.dest_path.exists() { "needs update" } else { "new" }
        } else {
            "synced"
        };
        
        println!("  {} [{}]", schema.repo, status);
        println!("    source: {}", schema.source_path.display());
        println!("    dest:   {}", schema.dest_path.display());
        println!();
    }
    
    Ok(())
}

fn handle_completions(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, APP_NAME, &mut io::stdout());
    Ok(())
}

// ============================================================================
// Utility Functions
// ============================================================================

fn load_config(paths: &AppPaths) -> Result<AppConfig> {
    if !paths.config_file.exists() {
        return Ok(AppConfig::default());
    }

    let built = Config::builder()
        .add_source(
            File::from(paths.config_file.as_path())
                .format(FileFormat::Toml)
                .required(false),
        )
        .add_source(Environment::with_prefix("BYT").separator("__"))
        .build()?;

    let config: AppConfig = built.try_deserialize().unwrap_or_default();
    Ok(config)
}

fn write_default_config(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory {parent:?}"))?;
    }

    let config = AppConfig::default();
    let toml = toml::to_string_pretty(&config).context("serializing default config to TOML")?;

    let mut body = String::new();
    body.push_str("# Configuration for byt - Byteowlz meta-tool\n");
    body.push_str(&format!("# File: {}\n\n", path.display()));
    body.push_str(&toml);

    fs::write(path, body).with_context(|| format!("writing config file to {}", path.display()))?;
    println!("Config written to: {}", path.display());
    Ok(())
}

fn expand_path(path: PathBuf) -> Result<PathBuf> {
    if let Some(text) = path.to_str() {
        let expanded = shellexpand::full(text).context("expanding path")?;
        Ok(PathBuf::from(expanded.to_string()))
    } else {
        Ok(path)
    }
}

fn default_config_dir() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir).join(APP_NAME));
    }

    if let Some(mut dir) = dirs::config_dir() {
        dir.push(APP_NAME);
        return Ok(dir);
    }

    dirs::home_dir()
        .map(|home| home.join(".config").join(APP_NAME))
        .ok_or_else(|| anyhow!("unable to determine configuration directory"))
}

fn discover_workspace_root() -> Result<PathBuf> {
    // First check if we're in a byteowlz workspace (has AGENTS.md with govnr markers)
    let current = env::current_dir()?;

    // Walk up looking for the workspace root
    let mut dir = current.as_path();
    loop {
        // Check for govnr markers
        let agents_md = dir.join("AGENTS.md");
        let catalog = dir.join("CATALOG.json");
        let beads = dir.join(".beads");

        if agents_md.exists() && beads.exists() {
            // Check if AGENTS.md mentions Govnr
            if let Ok(content) = fs::read_to_string(&agents_md) {
                if content.contains("Govnr") || content.contains("govnr") {
                    return Ok(dir.to_path_buf());
                }
            }
        }

        if catalog.exists() {
            return Ok(dir.to_path_buf());
        }

        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }

    // Fall back to current directory
    Ok(current)
}

impl fmt::Display for AppPaths {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "config: {}, workspace: {}",
            self.config_file.display(),
            self.workspace_root.display()
        )
    }
}
