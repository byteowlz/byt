use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use clap::{Args, CommandFactory, Parser, Subcommand};
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
        Command::Memory { command } => handle_memory(&ctx, command),
        Command::Sync { command } => handle_sync(&ctx, command),
        Command::Repos { command } => handle_repos(&ctx, command),
        Command::Init(cmd) => handle_init(&ctx, cmd),
        Command::Config { command } => handle_config(&ctx, command),
        Command::Secrets { command } => handle_secrets(&ctx, command),
        Command::New(cmd) => handle_new(&ctx, cmd),
        Command::Schema { command } => handle_schema(&ctx, command),
        Command::Website { command } => handle_website(&ctx, command),
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
    /// Manage repositories across machines
    Repos {
        #[command(subcommand)]
        command: ReposCommand,
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
    /// Manage byteowlz.com website (sync tool pages from repos)
    Website {
        #[command(subcommand)]
        command: WebsiteCommand,
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
    /// Show which repos exist on which machines
    Machines {
        #[command(subcommand)]
        command: MachinesSubcommand,
    },
}

#[derive(Debug, Subcommand)]
enum MachinesSubcommand {
    /// Show repos available on each machine
    Show {
        /// Only check specific machines (default: local + all configured remotes)
        #[arg(long, short = 'm')]
        machines: Vec<String>,
    },
    /// Compare repo availability across machines
    Compare {
        /// Only check specific machines (default: local + all configured remotes)
        #[arg(long, short = 'm')]
        machines: Vec<String>,
    },
    /// Show repos missing from local machine that exist on remotes
    Missing,
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

#[derive(Debug, Subcommand)]
enum ReposCommand {
    /// Sync all repositories and memories across machines
    Sync {
        /// Skip pulling changes from remote
        #[arg(long)]
        no_pull: bool,
        /// Push local commits to remote
        #[arg(long)]
        push: bool,
        /// Skip syncing memories via mmry export/import
        #[arg(long)]
        no_memories: bool,
        /// Only sync specific repos (default: all in catalog)
        #[arg(long, short = 'r')]
        repos: Vec<String>,
    },
    /// Show status of all repositories (uncommitted changes, ahead/behind)
    Status,
    /// Compare repository versions across configured machines
    Compare {
        /// Only compare specific repos
        #[arg(long, short = 'r')]
        repos: Vec<String>,
        /// Only check specific machines (default: all configured)
        #[arg(long, short = 'm')]
        machines: Vec<String>,
    },
    /// Clean build artifacts from Rust repositories
    Clean {
        /// Only clean specific repos (default: all Rust repos in catalog)
        #[arg(long, short = 'r')]
        repos: Vec<String>,
        /// Keep release builds
        #[arg(long)]
        keep_release: bool,
    },
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
    /// Clone from a git repository URL instead of using a template
    /// Can be a full URL (https://github.com/user/repo) or shorthand (user/repo for GitHub)
    #[arg(long, conflicts_with = "template")]
    from_git: Option<String>,
    /// When using --from-git, only use files from this subdirectory
    #[arg(long, requires = "from_git")]
    subdir: Option<String>,
    /// Git branch/tag/commit to clone (default: main or master)
    #[arg(long, requires = "from_git")]
    git_ref: Option<String>,
    /// Skip variable replacement in cloned repo files
    #[arg(long)]
    no_replace: bool,
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

#[derive(Debug, Subcommand)]
enum WebsiteCommand {
    /// Sync tool.toml files to website repository
    Sync {
        /// Commit changes after sync
        #[arg(long)]
        commit: bool,
        /// Only sync specific repos
        repos: Vec<String>,
    },
    /// List tools that have tool.toml files
    List,
    /// Check for tools that need website updates
    Check,
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

/// Machine-specific catalog showing which repos exist on a machine
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MachineCatalog {
    machine: String,
    generated: DateTime<Utc>,
    repos: Vec<String>,
}

/// Comparison of catalogs across multiple machines
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MachineCatalogComparison {
    generated: DateTime<Utc>,
    machines: Vec<String>,
    /// All repos across all machines with availability per machine
    repos: Vec<RepoAvailability>,
}

/// Availability of a single repo across machines
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RepoAvailability {
    name: String,
    /// Which machines have this repo (by machine name)
    present_on: Vec<String>,
    /// Which machines are missing this repo
    missing_on: Vec<String>,
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
        if common.workspace.is_none()
            && let Some(ref ws) = config.workspace
            && let Ok(expanded) = expand_path(PathBuf::from(ws))
        {
            paths.workspace_root = expanded;
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

    /// Load and parse the catalog with helpful error messages
    fn load_catalog(&self) -> Result<Catalog> {
        let catalog_path = self.catalog_path();
        if !catalog_path.exists() {
            anyhow::bail!(
                "Catalog not found at {}. Run 'byt catalog refresh' to generate it.",
                catalog_path.display()
            );
        }

        let content = fs::read_to_string(&catalog_path)
            .with_context(|| format!("reading catalog from {}", catalog_path.display()))?;

        // Check for merge conflict markers
        if content.contains("<<<<<<<") || content.contains(">>>>>>>") {
            anyhow::bail!(
                "Catalog file has merge conflicts: {}\n\n\
                 The catalog contains git merge conflict markers.\n\
                 Run 'byt catalog refresh' to regenerate it.",
                catalog_path.display()
            );
        }

        serde_json::from_str(&content).with_context(|| {
            format!(
                "Failed to parse catalog at {}.\n\
                 The file may be corrupted. Run 'byt catalog refresh' to regenerate it.",
                catalog_path.display()
            )
        })
    }
}

#[derive(Debug, Clone)]
struct AppPaths {
    config_file: PathBuf,
    workspace_root: PathBuf,
}

impl AppPaths {
    fn discover(
        config_override: Option<PathBuf>,
        workspace_override: Option<PathBuf>,
    ) -> Result<Self> {
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
    /// Website settings
    website: WebsiteConfig,
    /// Memory sync settings
    sync: SyncConfig,
    /// All machines in the ecosystem (byt auto-detects which is local via hostname)
    #[serde(default)]
    machines: MachinesConfig,
}

/// Machine configuration - a flat list of all machines in the ecosystem.
/// byt auto-detects which machine is local by matching hostname.
type MachinesConfig = Vec<Machine>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Machine {
    /// Machine name (for display)
    name: String,
    /// Hostname to match against (compared with `hostname` command output)
    /// If not set, uses `name` as the hostname
    #[serde(default)]
    hostname: Option<String>,
    /// SSH host (user@host or just host) - used when connecting remotely
    /// If not set, uses `hostname` (or `name` if hostname not set)
    #[serde(default)]
    host: Option<String>,
    /// SSH port (default: 22)
    #[serde(default = "default_ssh_port")]
    port: u16,
    /// Path to workspace on this machine
    #[serde(default)]
    workspace: Option<String>,
    /// SSH identity file (optional)
    #[serde(default)]
    identity_file: Option<String>,
}

impl Machine {
    /// Get the SSH host to connect to (falls back to hostname, then name)
    fn ssh_host(&self) -> &str {
        self.host
            .as_deref()
            .unwrap_or_else(|| self.hostname.as_deref().unwrap_or(&self.name))
    }
}

fn default_ssh_port() -> u16 {
    22
}

/// Get the XDG state directory for byt
fn get_state_dir() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir).join(APP_NAME));
    }

    dirs::home_dir()
        .map(|home| home.join(".local").join("state").join(APP_NAME))
        .ok_or_else(|| anyhow!("unable to determine state directory"))
}

/// Get the path to the machine identity state file
fn get_machine_state_path() -> Result<PathBuf> {
    Ok(get_state_dir()?.join("machine"))
}

/// Read the local machine name from state file
fn read_machine_state() -> Option<String> {
    get_machine_state_path()
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Write the local machine name to state file
fn write_machine_state(name: &str) -> Result<()> {
    let state_path = get_machine_state_path()?;
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&state_path, format!("{}\n", name))?;
    Ok(())
}

/// Get the current machine's hostname
fn get_current_hostname() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Try to auto-detect the local machine by fuzzy hostname matching
fn auto_detect_local_machine(machines: &[Machine]) -> Option<&Machine> {
    let current = get_current_hostname().to_lowercase();

    // Try exact match on hostname field first
    if let Some(m) = machines
        .iter()
        .find(|m| m.hostname.as_ref().map(|h| h.to_lowercase()) == Some(current.clone()))
    {
        return Some(m);
    }

    // Try if machine name is a prefix of current hostname (e.g., "macbook" matches "macbook.local")
    if let Some(m) = machines
        .iter()
        .find(|m| current.starts_with(&m.name.to_lowercase()))
    {
        return Some(m);
    }

    // Try if current hostname is a prefix of machine name
    if let Some(m) = machines
        .iter()
        .find(|m| m.name.to_lowercase().starts_with(&current))
    {
        return Some(m);
    }

    None
}

/// Prompt user to select their machine (interactive)
fn prompt_machine_selection(machines: &[Machine]) -> Result<String> {
    use std::io::{self, IsTerminal, Write};

    if machines.is_empty() {
        return Err(anyhow!("No machines configured in config.toml"));
    }

    // Check if stdin is a terminal - if not, we can't prompt
    if !io::stdin().is_terminal() {
        return Err(anyhow!(
            "Cannot determine local machine identity.\n\n\
             No machine state file found and stdin is not a terminal.\n\
             Run 'byt repos compare' interactively to set your machine identity,\n\
             or create {} with your machine name.",
            get_machine_state_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "~/.local/state/byt/machine".to_string())
        ));
    }

    eprintln!("No local machine identity found.\n");
    eprintln!("Available machines in config:");
    for (i, machine) in machines.iter().enumerate() {
        eprintln!("  {}. {}", i + 1, machine.name);
    }
    eprintln!();
    eprint!("Which machine is this? [1-{}]: ", machines.len());
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    let choice: usize = input
        .trim()
        .parse()
        .map_err(|_| anyhow!("Invalid selection"))?;

    if choice < 1 || choice > machines.len() {
        return Err(anyhow!("Selection out of range"));
    }

    let selected = &machines[choice - 1];

    // Save to state file
    write_machine_state(&selected.name)?;

    let state_path = get_machine_state_path()?;
    eprintln!("\nSaved to: {}", state_path.display());

    Ok(selected.name.clone())
}

/// Check if a machine is the local machine
fn is_local_machine(machine: &Machine, local_name: &str) -> bool {
    machine.name == local_name
}

/// Get the local machine name, prompting if necessary
fn get_local_machine_name_interactive(machines: &[Machine]) -> Result<String> {
    // 1. Check state file first
    if let Some(name) = read_machine_state() {
        // Verify it's still in config
        if machines.iter().any(|m| m.name == name) {
            return Ok(name);
        }
        // State file has invalid machine, will re-prompt
    }

    // 2. Try auto-detection by hostname
    if let Some(machine) = auto_detect_local_machine(machines) {
        // Auto-detected, save to state for next time
        let _ = write_machine_state(&machine.name);
        return Ok(machine.name.clone());
    }

    // 3. Prompt user
    prompt_machine_selection(machines)
}

/// Get the local machine name (non-interactive fallback)
fn get_local_machine_name(machines: &[Machine]) -> String {
    // Try state file
    if let Some(name) = read_machine_state() {
        if machines.iter().any(|m| m.name == name) {
            return name;
        }
    }

    // Try auto-detection
    if let Some(machine) = auto_detect_local_machine(machines) {
        return machine.name.clone();
    }

    // Fallback to hostname
    get_current_hostname()
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
            base_url: "https://raw.githubusercontent.com/byteowlz/schemas/refs/heads/main"
                .to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct WebsiteConfig {
    /// Path to website repository (relative to workspace or absolute)
    repo_path: String,
    /// Directory for tool pages within the website repo
    toolz_dir: String,
    /// Directory for tool assets (icons, screenshots)
    public_dir: String,
}

impl Default for WebsiteConfig {
    fn default() -> Self {
        Self {
            repo_path: "byteowlz.com".to_string(),
            toolz_dir: "pages/toolz".to_string(),
            public_dir: "public/toolz".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct SyncConfig {
    /// Directory for memory sync files (relative to workspace or absolute)
    /// Default: ".sync/memories" in workspace root
    /// Set to "govnr/.sync/memories" to keep sync files in the govnr repo
    sync_dir: String,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            sync_dir: ".sync/memories".to_string(),
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
            required_files: vec!["justfile".to_string(), ".beads".to_string()],
            release: ReleaseConfig::default(),
            templates: TemplatesConfig::default(),
            schemas: SchemaConfig::default(),
            website: WebsiteConfig::default(),
            sync: SyncConfig::default(),
            machines: Vec::new(),
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
        CatalogCommand::Machines { command } => handle_catalog_machines(ctx, command),
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
        info!(
            "dry-run: would write catalog to {}",
            ctx.catalog_path().display()
        );
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
        println!(
            "Catalog refreshed: {} repositories found",
            catalog.repos.len()
        );
        println!("Written to: {}", ctx.catalog_path().display());
    }

    Ok(())
}

fn show_catalog(ctx: &RuntimeContext) -> Result<()> {
    let catalog = ctx.load_catalog()?;

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

fn handle_catalog_machines(ctx: &RuntimeContext, command: MachinesSubcommand) -> Result<()> {
    match command {
        MachinesSubcommand::Show { machines } => catalog_machines_show(ctx, machines),
        MachinesSubcommand::Compare { machines } => catalog_machines_compare(ctx, machines),
        MachinesSubcommand::Missing => catalog_machines_missing(ctx),
    }
}

/// Get list of repos available on local machine (lightweight - just names)
fn get_local_repo_list(ctx: &RuntimeContext) -> Result<Vec<String>> {
    let repos = scan_repositories(ctx, false)?;
    let mut names: Vec<String> = repos.keys().cloned().collect();
    names.sort();
    Ok(names)
}

/// Get machine catalog for local machine
fn get_local_machine_catalog(ctx: &RuntimeContext, local_name: &str) -> Result<MachineCatalog> {
    let repos = get_local_repo_list(ctx)?;
    Ok(MachineCatalog {
        machine: local_name.to_string(),
        generated: Utc::now(),
        repos,
    })
}

/// Fetch machine catalog from remote via SSH
fn fetch_remote_machine_catalog(machine: &Machine) -> Result<MachineCatalog> {
    use std::process::Command as ShellCommand;

    let workspace_path = machine.workspace.as_deref().unwrap_or("~/byteowlz");

    // Build SSH command - use byt catalog list --json for lightweight output
    let port_str = machine.port.to_string();
    let ssh_host = machine.ssh_host();
    let remote_cmd = format!(
        "cd {} && $HOME/.cargo/bin/byt catalog list --json 2>/dev/null || byt catalog list --json",
        workspace_path
    );

    let mut ssh_args: Vec<&str> = vec!["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"];
    if machine.port != 22 {
        ssh_args.extend(["-p", &port_str]);
    }
    if let Some(ref identity) = machine.identity_file {
        ssh_args.extend(["-i", identity]);
    }
    ssh_args.push(ssh_host);
    ssh_args.push(&remote_cmd);

    let output = ShellCommand::new("ssh")
        .args(&ssh_args)
        .output()
        .with_context(|| format!("Failed to connect to {}", machine.name))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "SSH to {} failed: {}",
            machine.name,
            stderr.lines().next().unwrap_or("unknown error")
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let repos: Vec<String> = serde_json::from_str(&stdout)
        .with_context(|| format!("Failed to parse response from {}", machine.name))?;

    Ok(MachineCatalog {
        machine: machine.name.clone(),
        generated: Utc::now(),
        repos,
    })
}

/// Show repos available on each machine
fn catalog_machines_show(ctx: &RuntimeContext, explicit_machines: Vec<String>) -> Result<()> {
    let local_name = get_local_machine_name_interactive(&ctx.config.machines)?;

    // Get local catalog
    let local_catalog = get_local_machine_catalog(ctx, &local_name)?;

    // Determine which machines to query
    let machines: Vec<&Machine> = if !explicit_machines.is_empty() {
        ctx.config
            .machines
            .iter()
            .filter(|m| explicit_machines.contains(&m.name))
            .collect()
    } else {
        ctx.config.machines.iter().collect()
    };

    // Collect all catalogs
    let mut catalogs: Vec<MachineCatalog> = Vec::new();

    // Add local catalog first if not in explicit list or if it's included
    if explicit_machines.is_empty() || explicit_machines.contains(&local_name) {
        catalogs.push(local_catalog);
    }

    for machine in &machines {
        if is_local_machine(machine, &local_name) {
            // Skip - already added local catalog above (or will add if in explicit list)
            if !catalogs.iter().any(|c| c.machine == machine.name) {
                catalogs.push(MachineCatalog {
                    machine: machine.name.clone(),
                    generated: Utc::now(),
                    repos: get_local_repo_list(ctx)?,
                });
            }
            continue;
        }

        if !ctx.common.quiet {
            eprint!("Querying {}... ", machine.name);
        }
        match fetch_remote_machine_catalog(machine) {
            Ok(catalog) => {
                if !ctx.common.quiet {
                    eprintln!("OK ({} repos)", catalog.repos.len());
                }
                catalogs.push(catalog);
            }
            Err(e) => {
                if !ctx.common.quiet {
                    eprintln!("failed: {}", e);
                }
            }
        }
    }

    if ctx.common.json {
        println!("{}", serde_json::to_string_pretty(&catalogs)?);
    } else {
        println!("Machine Catalogs");
        println!("================\n");

        for catalog in &catalogs {
            println!("{} ({} repos):", catalog.machine, catalog.repos.len());
            for repo in &catalog.repos {
                println!("  {}", repo);
            }
            println!();
        }
    }

    Ok(())
}

/// Compare repo availability across machines
fn catalog_machines_compare(ctx: &RuntimeContext, explicit_machines: Vec<String>) -> Result<()> {
    use std::collections::{HashMap as StdHashMap, HashSet};

    let local_name = get_local_machine_name_interactive(&ctx.config.machines)?;

    // Get local catalog
    let local_catalog = get_local_machine_catalog(ctx, &local_name)?;

    // Determine which machines to query
    let machines: Vec<&Machine> = if !explicit_machines.is_empty() {
        ctx.config
            .machines
            .iter()
            .filter(|m| explicit_machines.contains(&m.name))
            .collect()
    } else {
        ctx.config.machines.iter().collect()
    };

    // Collect all catalogs
    let mut catalogs: StdHashMap<String, MachineCatalog> = StdHashMap::new();
    catalogs.insert(local_name.clone(), local_catalog);

    for machine in &machines {
        if is_local_machine(machine, &local_name) {
            // Already have local catalog
            continue;
        }

        if !ctx.common.quiet {
            eprint!("Querying {}... ", machine.name);
        }
        match fetch_remote_machine_catalog(machine) {
            Ok(catalog) => {
                if !ctx.common.quiet {
                    eprintln!("OK");
                }
                catalogs.insert(machine.name.clone(), catalog);
            }
            Err(e) => {
                if !ctx.common.quiet {
                    eprintln!("failed: {}", e);
                }
            }
        }
    }

    // Build ordered machine list (local first)
    let mut machine_names: Vec<String> = vec![local_name.clone()];
    for machine in &machines {
        if !is_local_machine(machine, &local_name) && catalogs.contains_key(&machine.name) {
            machine_names.push(machine.name.clone());
        }
    }

    // Collect all unique repos
    let mut all_repos: HashSet<String> = HashSet::new();
    for catalog in catalogs.values() {
        for repo in &catalog.repos {
            all_repos.insert(repo.clone());
        }
    }
    let mut all_repos: Vec<String> = all_repos.into_iter().collect();
    all_repos.sort();

    // Build availability data
    let mut repo_availability: Vec<RepoAvailability> = Vec::new();
    for repo in &all_repos {
        let mut present_on = Vec::new();
        let mut missing_on = Vec::new();

        for machine in &machine_names {
            if let Some(catalog) = catalogs.get(machine) {
                if catalog.repos.contains(repo) {
                    present_on.push(machine.clone());
                } else {
                    missing_on.push(machine.clone());
                }
            }
        }

        repo_availability.push(RepoAvailability {
            name: repo.clone(),
            present_on,
            missing_on,
        });
    }

    let comparison = MachineCatalogComparison {
        generated: Utc::now(),
        machines: machine_names.clone(),
        repos: repo_availability,
    };

    if ctx.common.json {
        println!("{}", serde_json::to_string_pretty(&comparison)?);
    } else {
        println!();
        println!("Repository Availability Across Machines");
        println!("========================================");
        println!();

        // Print header
        print!("{:<25}", "Repository");
        for name in &machine_names {
            print!(" {:>12}", name);
        }
        println!();
        print!("{:<25}", "-------------------------");
        for _ in &machine_names {
            print!(" {:>12}", "------------");
        }
        println!();

        // Print each repo
        for repo in &comparison.repos {
            print!("{:<25}", repo.name);
            for machine in &machine_names {
                if repo.present_on.contains(machine) {
                    print!(" {:>12}", "Y");
                } else {
                    print!(" {:>12}", "-");
                }
            }
            println!();
        }

        // Print summary
        println!();
        println!("Summary:");
        for machine in &machine_names {
            let count = comparison
                .repos
                .iter()
                .filter(|r| r.present_on.contains(machine))
                .count();
            println!("  {}: {} repos", machine, count);
        }
    }

    Ok(())
}

/// Show repos missing from local machine that exist on other machines
fn catalog_machines_missing(ctx: &RuntimeContext) -> Result<()> {
    use std::collections::HashSet;

    let local_name = get_local_machine_name_interactive(&ctx.config.machines)?;

    // Get local repos
    let local_repos: HashSet<String> = get_local_repo_list(ctx)?.into_iter().collect();

    // Get remote machines (all machines except local)
    let remote_machines: Vec<&Machine> = ctx
        .config
        .machines
        .iter()
        .filter(|m| !is_local_machine(m, &local_name))
        .collect();

    if remote_machines.is_empty() {
        println!("No other machines configured.");
        println!();
        println!("Add machines to ~/.config/byt/config.toml:");
        println!();
        println!("  [[machines]]");
        println!("  name = \"archvm\"");
        println!("  workspace = \"~/byteowlz\"");
        return Ok(());
    }

    // Collect repos from all remote machines
    let mut missing: Vec<(String, Vec<String>)> = Vec::new(); // (repo, machines_that_have_it)

    for machine in &remote_machines {
        if !ctx.common.quiet {
            eprint!("Querying {}... ", machine.name);
        }
        match fetch_remote_machine_catalog(machine) {
            Ok(catalog) => {
                if !ctx.common.quiet {
                    eprintln!("OK");
                }
                for repo in catalog.repos {
                    if !local_repos.contains(&repo) {
                        // Check if we already have this repo in missing list
                        if let Some(entry) = missing.iter_mut().find(|(r, _)| r == &repo) {
                            entry.1.push(machine.name.clone());
                        } else {
                            missing.push((repo, vec![machine.name.clone()]));
                        }
                    }
                }
            }
            Err(e) => {
                if !ctx.common.quiet {
                    eprintln!("failed: {}", e);
                }
            }
        }
    }

    missing.sort_by(|a, b| a.0.cmp(&b.0));

    if ctx.common.json {
        #[derive(Serialize)]
        struct MissingRepo {
            name: String,
            available_on: Vec<String>,
        }
        let output: Vec<MissingRepo> = missing
            .iter()
            .map(|(name, machines)| MissingRepo {
                name: name.clone(),
                available_on: machines.clone(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if missing.is_empty() {
        println!(
            "No repos missing from {} that exist on other machines.",
            local_name
        );
    } else {
        println!(
            "Repos missing from {} (available on other machines):",
            local_name
        );
        println!();
        for (repo, machines) in &missing {
            println!("  {} (on: {})", repo, machines.join(", "));
        }
        println!();
        println!("{} repo(s) missing locally.", missing.len());
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
    if let Ok(content) = fs::read_to_string(path.join("Cargo.toml"))
        && let Ok(cargo) = content.parse::<toml::Table>()
        && let Some(pkg) = cargo.get("package").and_then(|p| p.as_table())
        && let Some(desc) = pkg.get("description").and_then(|d| d.as_str())
    {
        return Some(desc.to_string());
    }

    // Try to get from package.json
    if let Ok(content) = fs::read_to_string(path.join("package.json"))
        && let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content)
        && let Some(desc) = pkg.get("description").and_then(|d| d.as_str())
    {
        return Some(desc.to_string());
    }

    // Try to get from pyproject.toml
    if let Ok(content) = fs::read_to_string(path.join("pyproject.toml"))
        && let Ok(pyproject) = content.parse::<toml::Table>()
        && let Some(project) = pyproject.get("project").and_then(|p| p.as_table())
        && let Some(desc) = project.get("description").and_then(|d| d.as_str())
    {
        return Some(desc.to_string());
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
        passed: issues
            .iter()
            .all(|i| !matches!(i.severity, Severity::Error)),
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
                println!(
                    "[{}] {} - {}: {}",
                    severity_icon, issue.repo, issue.rule, issue.message
                );
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

        let issues = info
            .open_issues
            .map(|n| format!(" ({} issues)", n))
            .unwrap_or_default();

        println!("[{}] {} [{}]{}", status_icon, name, compliance, issues);
    }

    println!();
    println!("Legend: J=justfile, B=beads, A=AGENTS.md");
    println!(
        "Active: {}, Stale: {}, Total: {}",
        active,
        stale,
        repos.len()
    );

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
        let issues: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap_or_default();

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
                let issue_type = issue
                    .get("issue_type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("task");
                println!("[P{}] {} ({}): {}", priority, id, issue_type, title);
            }
        }
    }

    Ok(())
}

fn handle_memory(ctx: &RuntimeContext, command: MemoryCommand) -> Result<()> {
    use std::process::Command;

    match command {
        MemoryCommand::Add {
            content,
            project,
            govnr,
            category,
            tags,
            importance,
        } => {
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
        MemoryCommand::Search {
            query,
            project,
            govnr,
            all,
            limit,
        } => {
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
        if let Some(first) = relative.components().next()
            && let Some(name) = first.as_os_str().to_str()
        {
            // Check if this is a valid repo in the catalog
            if let Ok(catalog) = ctx.load_catalog() {
                if catalog.repos.contains_key(name) {
                    info!("Auto-detected project: {}", name);
                    return Ok(name.to_string());
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
    let catalog = ctx.load_catalog()?;

    if catalog.repos.contains_key(project) {
        return Ok(project.to_string());
    }

    // Suggest similar names if not found
    let similar: Vec<&String> = catalog
        .repos
        .keys()
        .filter(|k| k.contains(project) || project.contains(k.as_str()))
        .take(3)
        .collect();

    if !similar.is_empty() {
        return Err(anyhow!(
            "Unknown project '{}'. Did you mean: {}?\nRun 'byt memory projects' to list available projects.",
            project,
            similar
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
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
    if stdout.contains(&format!("{} ", store_name))
        || stdout.contains(&format!("{} (default)", store_name))
    {
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
            return Err(anyhow!(
                "Failed to create store '{}': {}",
                store_name,
                stderr
            ));
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
        .filter_map(|l| l.split_whitespace().next())
        .collect();

    // Get catalog repos
    let catalog_repos: Vec<String> = match ctx.load_catalog() {
        Ok(catalog) => catalog.repos.keys().cloned().collect(),
        Err(_) => Vec::new(),
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
            println!(
                "  ... and {} more (run 'byt catalog list' for full list)",
                catalog_repos.len() - 20
            );
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
    if let Ok(catalog) = ctx.load_catalog() {
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
        .filter_map(|l| l.split_whitespace().next())
        .map(|s| s.to_string())
        .collect();

    Ok(stores)
}

/// Export memories to sync directory for git-based sync
fn sync_push(ctx: &RuntimeContext, explicit_stores: Vec<String>) -> Result<()> {
    use std::process::Command;

    let stores = get_syncable_stores(ctx, explicit_stores)?;
    let sync_dir = resolve_sync_dir(ctx)?;

    fs::create_dir_all(&sync_dir)?;

    if ctx.common.dry_run {
        println!(
            "Would export {} stores to {}",
            stores.len(),
            sync_dir.display()
        );
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
            .args([
                "export",
                "--store",
                store,
                "-o",
                &output_file.display().to_string(),
            ])
            .output()
            .with_context(|| format!("exporting store '{}'", store))?;

        if output.status.success() {
            // Count memories in export
            if let Ok(content) = fs::read_to_string(&output_file)
                && let Ok(export) = serde_json::from_str::<serde_json::Value>(&content)
            {
                let count = export
                    .get("memory_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                total_memories += count;
                if !ctx.common.quiet {
                    println!("  {} ({} memories)", store, count);
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
        println!(
            "{}",
            serde_json::to_string_pretty(&PushResult {
                stores_exported: exported,
                total_memories,
                sync_dir: sync_dir.display().to_string(),
            })?
        );
    } else {
        println!();
        println!(
            "Exported {} stores ({} memories) to {}",
            exported,
            total_memories,
            sync_dir.display()
        );
        println!("Run 'git add .sync && git commit' to sync");
    }

    Ok(())
}

/// Import memories from sync directory after git pull
fn sync_pull(ctx: &RuntimeContext, explicit_stores: Vec<String>) -> Result<()> {
    use std::process::Command;

    let sync_dir = resolve_sync_dir(ctx)?;

    if !sync_dir.exists() {
        return Err(anyhow!(
            "Sync directory not found: {}. Run 'byt sync push' first or 'git pull'.",
            sync_dir.display()
        ));
    }

    // Get stores to import
    let stores = if explicit_stores.is_empty() {
        // Auto-detect from files in sync dir that match local repos
        let local_repos: Vec<String> = match ctx.load_catalog() {
            Ok(catalog) => catalog.repos.keys().cloned().collect(),
            Err(_) => Vec::new(),
        };

        let mut stores = Vec::new();
        for entry in fs::read_dir(&sync_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                // Always sync govnr, or sync if repo exists locally
                if stem == "govnr" || local_repos.contains(&stem.to_string()) {
                    stores.push(stem.to_string());
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
        println!(
            "Would import {} stores from {}",
            stores.len(),
            sync_dir.display()
        );
        for store in &stores {
            println!("  - {}", store);
        }
        return Ok(());
    }

    let mut imported_count = 0;

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

        // Use mmry import command (handles HMLR data, deduplication, re-embedding)
        let output = Command::new("mmry")
            .args([
                "import",
                &input_file.display().to_string(),
                "--store",
                store,
            ])
            .output()
            .context("running mmry import")?;

        if !ctx.common.quiet {
            // Parse output for summary
            let stdout = String::from_utf8_lossy(&output.stdout);
            if output.status.success() {
                // Extract key info from output
                let lines: Vec<&str> = stdout.lines().collect();
                let summary = lines
                    .iter()
                    .find(|l| l.contains("memories") || l.contains("Import complete"))
                    .map(|s| s.trim())
                    .unwrap_or("imported");
                println!("  {} - {}", store, summary);
                imported_count += 1;
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!(
                    "  {} - error: {}",
                    store,
                    stderr.lines().next().unwrap_or("unknown error")
                );
            }
        } else if output.status.success() {
            imported_count += 1;
        }
    }

    if ctx.common.json {
        #[derive(Serialize)]
        struct PullResult {
            stores_imported: usize,
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&PullResult {
                stores_imported: imported_count,
            })?
        );
    } else {
        println!();
        println!("Imported {} stores (with HMLR data)", imported_count);
    }

    Ok(())
}

/// Resolve the sync directory from config
fn resolve_sync_dir(ctx: &RuntimeContext) -> Result<PathBuf> {
    let sync_path = &ctx.config.sync.sync_dir;

    // If absolute path, use as-is
    if sync_path.starts_with('/') || sync_path.starts_with('~') {
        let expanded = shellexpand::full(sync_path).context("expanding sync_dir path")?;
        return Ok(PathBuf::from(expanded.to_string()));
    }

    // Otherwise, relative to workspace
    Ok(ctx.workspace_root().join(sync_path))
}

/// Show sync status
fn sync_status(ctx: &RuntimeContext) -> Result<()> {
    let sync_dir = resolve_sync_dir(ctx)?;
    let existing_stores = get_existing_stores()?;

    // Get catalog repos
    let catalog_repos: Vec<String> = match ctx.load_catalog() {
        Ok(catalog) => catalog.repos.keys().cloned().collect(),
        Err(_) => Vec::new(),
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
                .filter_map(|e| {
                    e.path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                })
                .collect()
        } else {
            Vec::new()
        };

        let syncable: Vec<String> = existing_stores
            .iter()
            .filter(|s| *s == "govnr" || catalog_repos.contains(s))
            .cloned()
            .collect();

        println!(
            "{}",
            serde_json::to_string_pretty(&SyncStatus {
                sync_dir_exists: sync_dir.exists(),
                sync_files,
                local_stores: existing_stores,
                syncable_stores: syncable,
            })?
        );
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
            let marker = if syncable {
                "[syncable]"
            } else {
                "[local-only]"
            };
            println!("  {} {}", store, marker);
        }

        println!();
        println!("Commands:");
        println!("  byt sync push    # Export memories to .sync/");
        println!("  byt sync pull    # Import memories from .sync/");
    }

    Ok(())
}

// ============================================================================
// Repos Command Handlers
// ============================================================================

fn handle_repos(ctx: &RuntimeContext, command: ReposCommand) -> Result<()> {
    match command {
        ReposCommand::Sync {
            no_pull,
            push,
            no_memories,
            repos,
        } => repos_sync(ctx, !no_pull, push, !no_memories, repos),
        ReposCommand::Status => repos_status(ctx),
        ReposCommand::Compare { repos, machines } => repos_compare(ctx, repos, machines),
        ReposCommand::Clean {
            repos,
            keep_release,
        } => repos_clean(ctx, repos, keep_release),
    }
}

/// Sync all repositories and memories across machines
fn repos_sync(
    ctx: &RuntimeContext,
    do_pull: bool,
    do_push: bool,
    sync_memories: bool,
    explicit_repos: Vec<String>,
) -> Result<()> {
    use std::process::Command as ShellCommand;

    let workspace = ctx.workspace_root();

    // Load catalog to get all repos
    let repos: Vec<String> = if !explicit_repos.is_empty() {
        explicit_repos
    } else if let Ok(catalog) = ctx.load_catalog() {
        catalog.repos.keys().cloned().collect()
    } else {
        // Fallback: scan workspace for git repos
        let mut found = Vec::new();
        for entry in fs::read_dir(workspace)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir()
                && path.join(".git").exists()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                found.push(name.to_string());
            }
        }
        found
    };

    if repos.is_empty() {
        println!("No repositories found. Run 'byt catalog refresh' first.");
        return Ok(());
    }

    println!("Syncing {} repositories...\n", repos.len());

    let mut pull_ok = 0;
    let mut pull_failed = 0;
    let mut push_ok = 0;
    let mut push_failed = 0;
    let mut has_changes = Vec::new();
    let mut needs_attention = Vec::new(); // Repos with conflicts or other issues

    for repo_name in &repos {
        let repo_path = workspace.join(repo_name);
        if !repo_path.exists() {
            if !ctx.common.quiet {
                println!("  {} - skipped (not found locally)", repo_name);
            }
            continue;
        }

        if !repo_path.join(".git").exists() {
            if !ctx.common.quiet {
                println!("  {} - skipped (not a git repo)", repo_name);
            }
            continue;
        }

        // Check for uncommitted changes first
        let status_output = ShellCommand::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&repo_path)
            .output()?;
        let has_uncommitted = !status_output.stdout.is_empty();

        if has_uncommitted {
            has_changes.push(repo_name.clone());
        }

        // Pull
        if do_pull {
            if ctx.common.dry_run {
                println!("  {} - would pull", repo_name);
            } else {
                let output = ShellCommand::new("git")
                    .args(["pull", "--rebase", "--autostash"])
                    .current_dir(&repo_path)
                    .output()?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = format!("{}{}", stdout, stderr);

                // Check for conflict indicators (even if exit code is 0)
                let has_conflict = combined.contains("CONFLICT")
                    || combined.contains("could not apply")
                    || combined.contains("Applying autostash resulted in conflicts")
                    || combined.contains("needs merge")
                    || combined.contains("fix conflicts");

                if output.status.success() && !has_conflict {
                    let summary = if stdout.contains("Already up to date") {
                        "up to date"
                    } else if stdout.contains("Fast-forward") {
                        "fast-forward"
                    } else {
                        "pulled"
                    };
                    if !ctx.common.quiet {
                        println!("  {} - {}", repo_name, summary);
                    }
                    pull_ok += 1;
                } else if has_conflict {
                    // Conflict detected (might still have exit code 0 for autostash conflicts)
                    eprintln!(
                        "  {} - CONFLICT detected, manual resolution required",
                        repo_name
                    );
                    needs_attention.push((repo_name.clone(), "merge conflict".to_string()));
                    pull_failed += 1;
                } else {
                    // Other failure
                    let error_msg: String = stderr.lines().take(3).collect::<Vec<_>>().join(" | ");
                    eprintln!(
                        "  {} - pull failed: {}",
                        repo_name,
                        if error_msg.is_empty() {
                            "unknown error".to_string()
                        } else {
                            error_msg
                        }
                    );
                    needs_attention.push((repo_name.clone(), "pull failed".to_string()));
                    pull_failed += 1;
                }
            }
        }

        // Push (only if requested and there are commits to push)
        if do_push && !ctx.common.dry_run {
            // Check if there are commits to push
            let ahead_output = ShellCommand::new("git")
                .args(["rev-list", "--count", "@{upstream}..HEAD"])
                .current_dir(&repo_path)
                .output();

            if let Ok(output) = ahead_output {
                let ahead_count: i32 = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse()
                    .unwrap_or(0);

                if ahead_count > 0 {
                    let push_output = ShellCommand::new("git")
                        .args(["push"])
                        .current_dir(&repo_path)
                        .output()?;

                    if push_output.status.success() {
                        if !ctx.common.quiet {
                            println!("  {} - pushed {} commit(s)", repo_name, ahead_count);
                        }
                        push_ok += 1;
                    } else {
                        let stderr = String::from_utf8_lossy(&push_output.stderr);
                        let error_msg: String =
                            stderr.lines().take(3).collect::<Vec<_>>().join(" | ");
                        eprintln!(
                            "  {} - push failed: {}",
                            repo_name,
                            if error_msg.is_empty() {
                                "unknown error".to_string()
                            } else {
                                error_msg
                            }
                        );
                        needs_attention.push((repo_name.clone(), "push failed".to_string()));
                        push_failed += 1;
                    }
                }
            }
        }
    }

    // Sync memories if requested
    if sync_memories && !ctx.common.dry_run {
        println!();
        println!("Syncing memories...");

        // Get stores that match repos
        let memory_stores: Vec<String> = repos
            .iter()
            .filter(|r| {
                // Check if store exists
                ShellCommand::new("mmry")
                    .args(["stores", "info", r])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        // Always include govnr
        let mut stores_to_sync = memory_stores;
        if !stores_to_sync.contains(&"govnr".to_string()) {
            stores_to_sync.insert(0, "govnr".to_string());
        }

        if do_pull {
            // Import from sync dir (after git pull brought in new files)
            let sync_dir = resolve_sync_dir(ctx)?;
            if sync_dir.exists() {
                for store in &stores_to_sync {
                    let import_file = sync_dir.join(format!("{}.json", store));
                    if import_file.exists() {
                        let output = ShellCommand::new("mmry")
                            .args([
                                "import",
                                &import_file.display().to_string(),
                                "--store",
                                store,
                            ])
                            .output();

                        match output {
                            Ok(o) if o.status.success() => {
                                if !ctx.common.quiet {
                                    println!("  {} - imported", store);
                                }
                            }
                            Ok(o) => {
                                let stderr = String::from_utf8_lossy(&o.stderr);
                                if !ctx.common.quiet {
                                    println!(
                                        "  {} - import warning: {}",
                                        store,
                                        stderr.lines().next().unwrap_or("")
                                    );
                                }
                            }
                            Err(e) => {
                                eprintln!("  {} - import error: {}", store, e);
                            }
                        }
                    }
                }
            }
        }

        if do_push {
            // Export to sync dir
            let sync_dir = resolve_sync_dir(ctx)?;
            fs::create_dir_all(&sync_dir)?;

            for store in &stores_to_sync {
                let output_file = sync_dir.join(format!("{}.json", store));
                let output = ShellCommand::new("mmry")
                    .args([
                        "export",
                        "--store",
                        store,
                        "-o",
                        &output_file.display().to_string(),
                    ])
                    .output();

                match output {
                    Ok(o) if o.status.success() => {
                        if !ctx.common.quiet {
                            println!("  {} - exported", store);
                        }
                    }
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        eprintln!(
                            "  {} - export failed: {}",
                            store,
                            stderr.lines().next().unwrap_or("")
                        );
                    }
                    Err(e) => {
                        eprintln!("  {} - export error: {}", store, e);
                    }
                }
            }
        }
    }

    // Summary
    println!();
    if do_pull {
        println!("Pull: {} ok, {} failed", pull_ok, pull_failed);
    }
    if do_push {
        println!("Push: {} ok, {} failed", push_ok, push_failed);
    }

    // Show repos needing attention (conflicts, failures)
    if !needs_attention.is_empty() {
        println!();
        println!("Repos requiring attention:");
        for (repo, reason) in &needs_attention {
            println!("  - {} ({})", repo, reason);
        }
        println!();
        println!("To resolve conflicts:");
        println!("  cd <repo> && git status    # See conflict details");
        println!("  git diff                   # Review conflicts");
        println!("  # Edit files to resolve, then:");
        println!("  git add <files> && git rebase --continue");
    }

    if !has_changes.is_empty() {
        println!();
        println!("Repos with uncommitted changes:");
        for repo in &has_changes {
            println!("  - {}", repo);
        }
    }

    if do_push && sync_memories {
        println!();
        println!("Don't forget to commit and push the .sync/ directory:");
        println!(
            "  cd ~/byteowlz/govnr && git add .sync && git commit -m 'sync memories' && git push"
        );
    }

    Ok(())
}

/// Repository status info for JSON output
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RepoStatusInfo {
    name: String,
    exists: bool,
    head_commit: Option<String>,
    head_date: Option<String>,
    branch: Option<String>,
    uncommitted: i32,
    ahead: i32,
    behind: i32,
}

/// Machine status response
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MachineStatus {
    machine: String,
    repos: Vec<RepoStatusInfo>,
}

/// Get detailed status for a single repo
fn get_repo_status(repo_path: &Path, repo_name: &str) -> RepoStatusInfo {
    use std::process::Command as ShellCommand;

    if !repo_path.exists() || !repo_path.join(".git").exists() {
        return RepoStatusInfo {
            name: repo_name.to_string(),
            exists: false,
            head_commit: None,
            head_date: None,
            branch: None,
            uncommitted: 0,
            ahead: 0,
            behind: 0,
        };
    }

    // Get HEAD commit hash
    let head_commit = ShellCommand::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    // Get HEAD commit date (ISO format)
    let head_date = ShellCommand::new("git")
        .args(["log", "-1", "--format=%cI"])
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    // Get current branch
    let branch = ShellCommand::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    // Get uncommitted count
    let status_output = ShellCommand::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_path)
        .output();
    let uncommitted = status_output
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count() as i32)
        .unwrap_or(0);

    // Get ahead count
    let ahead = ShellCommand::new("git")
        .args(["rev-list", "--count", "@{upstream}..HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse()
                .unwrap_or(0)
        })
        .unwrap_or(0);

    // Get behind count
    let behind = ShellCommand::new("git")
        .args(["rev-list", "--count", "HEAD..@{upstream}"])
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse()
                .unwrap_or(0)
        })
        .unwrap_or(0);

    RepoStatusInfo {
        name: repo_name.to_string(),
        exists: true,
        head_commit,
        head_date,
        branch,
        uncommitted,
        ahead,
        behind,
    }
}

/// Show status of all repositories
fn repos_status(ctx: &RuntimeContext) -> Result<()> {
    let workspace = ctx.workspace_root();

    let repos: Vec<String> = match ctx.load_catalog() {
        Ok(catalog) => catalog.repos.keys().cloned().collect(),
        Err(e) => {
            if ctx.common.json {
                println!("[]");
            } else {
                eprintln!("{}", e);
            }
            return Ok(());
        }
    };

    if repos.is_empty() {
        if ctx.common.json {
            println!("[]");
        } else {
            println!("No repositories in catalog. Run 'byt catalog refresh' first.");
        }
        return Ok(());
    }

    let statuses: Vec<RepoStatusInfo> = repos
        .iter()
        .map(|name| get_repo_status(&workspace.join(name), name))
        .collect();

    if ctx.common.json {
        let machine_status = MachineStatus {
            machine: get_local_machine_name(&ctx.config.machines),
            repos: statuses,
        };
        println!("{}", serde_json::to_string_pretty(&machine_status)?);
    } else {
        println!("Repository Status");
        println!("=================\n");

        for status in &statuses {
            if !status.exists {
                println!("  {} - not found", status.name);
                continue;
            }

            let mut parts = Vec::new();
            if status.uncommitted > 0 {
                parts.push(format!("{} uncommitted", status.uncommitted));
            }
            if status.ahead > 0 {
                parts.push(format!("{} ahead", status.ahead));
            }
            if status.behind > 0 {
                parts.push(format!("{} behind", status.behind));
            }

            if parts.is_empty() {
                println!(
                    "  {} - clean ({})",
                    status.name,
                    status.head_commit.as_deref().unwrap_or("?")
                );
            } else {
                println!(
                    "  {} - {} ({})",
                    status.name,
                    parts.join(", "),
                    status.head_commit.as_deref().unwrap_or("?")
                );
            }
        }
    }

    Ok(())
}

/// Compare repositories across configured machines
fn repos_compare(
    ctx: &RuntimeContext,
    explicit_repos: Vec<String>,
    explicit_machines: Vec<String>,
) -> Result<()> {
    use std::collections::HashMap;
    use std::process::Command as ShellCommand;

    let workspace = ctx.workspace_root();
    let local_name = get_local_machine_name_interactive(&ctx.config.machines)?;

    // Get repos to compare
    let repos: Vec<String> = if !explicit_repos.is_empty() {
        explicit_repos
    } else {
        let catalog = ctx.load_catalog()?;
        catalog.repos.keys().cloned().collect()
    };

    if repos.is_empty() {
        println!("No repositories to compare. Run 'byt catalog refresh' first.");
        return Ok(());
    }

    // Get machines to query (all except local)
    let remote_machines: Vec<&Machine> = if !explicit_machines.is_empty() {
        ctx.config
            .machines
            .iter()
            .filter(|m| explicit_machines.contains(&m.name) && !is_local_machine(m, &local_name))
            .collect()
    } else {
        ctx.config
            .machines
            .iter()
            .filter(|m| !is_local_machine(m, &local_name))
            .collect()
    };

    if remote_machines.is_empty()
        && ctx
            .config
            .machines
            .iter()
            .filter(|m| !is_local_machine(m, &local_name))
            .count()
            == 0
    {
        println!("No other machines configured.");
        println!();
        println!("Add machines to ~/.config/byt/config.toml:");
        println!();
        println!("  [[machines]]");
        println!("  name = \"archvm\"");
        println!("  workspace = \"~/byteowlz\"");
        return Ok(());
    }

    // Collect status from local machine
    let local_statuses: Vec<RepoStatusInfo> = repos
        .iter()
        .map(|name| get_repo_status(&workspace.join(name), name))
        .collect();

    let mut all_statuses: HashMap<String, MachineStatus> = HashMap::new();
    all_statuses.insert(
        local_name.clone(),
        MachineStatus {
            machine: local_name.clone(),
            repos: local_statuses,
        },
    );

    // Query remote machines via SSH
    for machine in &remote_machines {
        if !ctx.common.quiet {
            eprint!("Querying {}... ", machine.name);
        }

        let workspace_path = machine.workspace.as_deref().unwrap_or("~/byteowlz");

        // Build SSH command
        let port_str = machine.port.to_string();
        let ssh_host = machine.ssh_host();
        // Use login shell to ensure PATH includes ~/.cargo/bin etc.
        let remote_cmd = format!(
            "cd {} && $HOME/.cargo/bin/byt repos status --json 2>/dev/null || byt repos status --json",
            workspace_path
        );

        let mut ssh_args: Vec<&str> = vec!["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"];
        if machine.port != 22 {
            ssh_args.extend(["-p", &port_str]);
        }
        if let Some(ref identity) = machine.identity_file {
            ssh_args.extend(["-i", identity]);
        }
        ssh_args.push(ssh_host);
        ssh_args.push(&remote_cmd);

        let output = ShellCommand::new("ssh").args(&ssh_args).output();

        match output {
            Ok(o) if o.status.success() => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                match serde_json::from_str::<MachineStatus>(&stdout) {
                    Ok(status) => {
                        if !ctx.common.quiet {
                            eprintln!("OK");
                        }
                        all_statuses.insert(machine.name.clone(), status);
                    }
                    Err(e) => {
                        if !ctx.common.quiet {
                            eprintln!("parse error: {}", e);
                        }
                    }
                }
            }
            Ok(o) => {
                if !ctx.common.quiet {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    eprintln!(
                        "failed: {}",
                        stderr.lines().next().unwrap_or("unknown error")
                    );
                }
            }
            Err(e) => {
                if !ctx.common.quiet {
                    eprintln!("error: {}", e);
                }
            }
        }
    }

    if ctx.common.json {
        println!("{}", serde_json::to_string_pretty(&all_statuses)?);
        return Ok(());
    }

    // Build comparison table
    println!();
    println!("Repository Comparison Across Machines");
    println!("=====================================");
    println!();

    // Get all machine names in order (local first)
    let mut machine_names: Vec<String> = vec![local_name.clone()];
    for machine in &remote_machines {
        if all_statuses.contains_key(&machine.name) {
            machine_names.push(machine.name.clone());
        }
    }

    // Print header
    print!("{:<20}", "Repository");
    for name in &machine_names {
        print!(" {:>15}", name);
    }
    println!();
    print!("{:<20}", "----------");
    for _ in &machine_names {
        print!(" {:>15}", "---------------");
    }
    println!();

    // Print each repo
    for repo_name in &repos {
        print!("{:<20}", repo_name);

        let mut commits: Vec<Option<&str>> = Vec::new();
        let mut dates: Vec<Option<&str>> = Vec::new();

        for machine_name in &machine_names {
            if let Some(status) = all_statuses.get(machine_name) {
                if let Some(repo) = status.repos.iter().find(|r| &r.name == repo_name) {
                    if repo.exists {
                        let commit = repo.head_commit.as_deref().unwrap_or("?");
                        let uncommitted = if repo.uncommitted > 0 {
                            format!("*{}", commit)
                        } else {
                            commit.to_string()
                        };
                        print!(" {:>15}", uncommitted);
                        commits.push(repo.head_commit.as_deref());
                        dates.push(repo.head_date.as_deref());
                    } else {
                        print!(" {:>15}", "-");
                        commits.push(None);
                        dates.push(None);
                    }
                } else {
                    print!(" {:>15}", "-");
                    commits.push(None);
                    dates.push(None);
                }
            } else {
                print!(" {:>15}", "?");
                commits.push(None);
                dates.push(None);
            }
        }

        // Determine which machine has the newest version
        let newest_idx = dates
            .iter()
            .enumerate()
            .filter_map(|(i, d)| d.map(|date| (i, date)))
            .max_by_key(|(_, date)| *date)
            .map(|(i, _)| i);

        // Check if all commits match
        let all_same = commits
            .iter()
            .filter_map(|c| *c)
            .collect::<Vec<_>>()
            .windows(2)
            .all(|w| w[0] == w[1]);

        if !all_same && let Some(idx) = newest_idx {
            print!("  <- {}", machine_names[idx]);
        }

        println!();
    }

    println!();
    println!("Legend: * = has uncommitted changes, <- = newest version");

    Ok(())
}

/// Recursively count files in a directory
fn count_files(dir: &Path) -> Result<usize> {
    let mut count = 0;
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            count += 1;
        }
    }
    Ok(count)
}

/// Clean build artifacts from Rust repositories
fn repos_clean(
    ctx: &RuntimeContext,
    explicit_repos: Vec<String>,
    keep_release: bool,
) -> Result<()> {
    use std::process::Command as ShellCommand;

    let workspace = ctx.workspace_root();

    // Get repos to clean
    let repos: Vec<String> = if !explicit_repos.is_empty() {
        explicit_repos
    } else if let Ok(catalog) = ctx.load_catalog() {
        // Filter to Rust repos only
        catalog
            .repos
            .iter()
            .filter(|(_, info)| info.languages.contains(&"rust".to_string()))
            .map(|(name, _)| name.clone())
            .collect()
    } else {
        // Fallback: scan for Rust workspaces
        scan_repositories(ctx, false)?
            .iter()
            .filter(|(_, info)| info.languages.contains(&"rust".to_string()))
            .map(|(name, _)| name.clone())
            .collect()
    };

    if repos.is_empty() {
        println!("No Rust repositories to clean.");
        return Ok(());
    }

    let mut total_freed = 0u64;
    let mut total_files = 0usize;

    for repo_name in &repos {
        let repo_path = workspace.join(repo_name);

        if !repo_path.exists() {
            if !ctx.common.quiet {
                eprintln!("Skipping: {} (not found)", repo_name);
            }
            continue;
        }

        // Check if this is a workspace
        let cargo_toml = repo_path.join("Cargo.toml");
        if !cargo_toml.exists() {
            continue; // Not a Rust project
        }

        // Read Cargo.toml to check if it's a workspace
        let is_workspace = fs::read_to_string(&cargo_toml)
            .ok()
            .and_then(|content| {
                content.parse::<toml::Table>().ok().and_then(|toml| {
                    toml.get("workspace")
                        .and_then(|w| w.as_table())
                        .map(|_| true)
                })
            })
            .unwrap_or(false);

        if !is_workspace {
            // Single crate - check if it has a target directory
            if !repo_path.join("target").exists() {
                continue; // Nothing to clean
            }
        }

        if !ctx.common.quiet {
            eprint!("Cleaning: {}... ", repo_name);
        }

        let target_dir = repo_path.join("target");

        let mut freed_space: u64 = 0;
        let mut removed_files: usize = 0;

        if keep_release {
            // Only clean debug builds
            let debug_dir = target_dir.join("debug");
            if debug_dir.exists() {
                let output = ShellCommand::new("du")
                    .args(["-sb", debug_dir.to_str().unwrap()])
                    .output()
                    .ok();

                if let Some(ref o) = output {
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    if let Some(size_str) = stdout.split_whitespace().next() {
                        freed_space = size_str.parse::<u64>().unwrap_or(0);
                    }
                }

                // Remove debug directory
                if let Ok(count) = count_files(&debug_dir) {
                    removed_files = count;
                }

                if ctx.common.dry_run {
                    if !ctx.common.quiet {
                        let mib = freed_space / 1024 / 1024;
                        eprintln!("dry-run (would remove debug: {} files, {} MiB)", removed_files, mib);
                    }
                } else {
                    fs::remove_dir_all(&debug_dir)
                        .with_context(|| format!("Failed to remove debug directory in {}", repo_name))?;
                    if !ctx.common.quiet {
                        let mib = freed_space / 1024 / 1024;
                        eprintln!("✓ Removed debug ({} files, {} MiB)", removed_files, mib);
                    }
                }
            } else {
                if !ctx.common.quiet {
                    eprintln!("✓ no debug artifacts to clean");
                }
            }
        } else {
            // Clean everything (default)
            let cargo_clean_args = vec!["clean", "--manifest-path", cargo_toml.to_str().unwrap()];

            if ctx.common.dry_run {
                if !ctx.common.quiet {
                    eprintln!("dry-run");
                }
                continue;
            }

            let output = ShellCommand::new("cargo")
                .args(&cargo_clean_args)
                .output()
                .with_context(|| format!("Failed to run cargo clean in {}", repo_name))?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                // Try stdout first, then stderr for "Removed" line
                let output_text = if stdout.contains("Removed") {
                    &stdout
                } else if stderr.contains("Removed") {
                    &stderr
                } else {
                    &stdout
                };

                if let Some(line) = output_text.lines().find(|l| l.contains("Removed")) {
                    if !ctx.common.quiet {
                        eprintln!("✓ {}", line.trim());
                    }
                    // Parse freed space
                    if let Some(freed_str) = line.split_whitespace().find(|s| {
                        s.ends_with("GiB") || s.ends_with("MiB") || s.ends_with("KiB")
                    }) {
                        let num_str = freed_str.trim_end_matches("GiB").trim_end_matches("MiB").trim_end_matches("KiB");
                        if let Ok(num) = num_str.parse::<f64>() {
                            total_freed += if freed_str.ends_with("GiB") {
                                (num * 1024.0 * 1024.0 * 1024.0) as u64  // GiB to bytes
                            } else if freed_str.ends_with("MiB") {
                                (num * 1024.0 * 1024.0) as u64  // MiB to bytes
                            } else {
                                (num * 1024.0) as u64  // KiB to bytes
                            };
                        }
                    }
                    if let Some(files_str) = line.split_whitespace().next() {
                        if let Ok(files) = files_str.parse::<usize>() {
                            total_files += files;
                        }
                    }
                } else {
                    if !ctx.common.quiet {
                        eprintln!("✓ cleaned (no target dir)");
                    }
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !ctx.common.quiet {
                    eprintln!("✗ {}", stderr.lines().next().unwrap_or("unknown error"));
                }
            }
        }

        // Track totals for both modes
        total_freed += freed_space;
        total_files += removed_files;
    }

    // Print summary
    if !ctx.common.quiet {
        println!();
        if total_freed > 0 {
            let gib = total_freed as f64 / 1024.0 / 1024.0 / 1024.0;
            let mib = total_freed as f64 / 1024.0 / 1024.0;
            if gib >= 1.0 {
                println!("Total space freed: {:.1} GiB", gib);
            } else if mib >= 1.0 {
                println!("Total space freed: {:.1} MiB", mib);
            } else {
                println!("Total space freed: {} KiB", total_freed / 1024);
            }
        }
        if total_files > 0 {
            println!("Total files removed: {}", total_files);
        }
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
    let gh_check = Command::new("gh").arg("--version").output();

    if gh_check.is_err() {
        return Err(anyhow!(
            "GitHub CLI (gh) not found. Install it from https://cli.github.com"
        ));
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
                println!(
                    "This token needs 'repo' scope for your homebrew-tap and scoop-bucket repos"
                );
            } else {
                println!(
                    "This token needs 'repo' scope for: {}",
                    needs_access.join(", ")
                );
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
                println!(
                    "{} is already set. Skipping (use 'byt secrets set' to update)",
                    token_name
                );
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

                let has_aur_key = existing
                    .lines()
                    .any(|l| l.starts_with("AUR_SSH_PRIVATE_KEY"));

                if has_aur_key {
                    println!("AUR_SSH_PRIVATE_KEY is already set. Skipping.");
                } else if let Some(ref key_path) = ctx.config.release.aur_ssh_key_path {
                    let expanded = shellexpand::tilde(key_path);
                    let key_file = Path::new(expanded.as_ref());

                    if key_file.exists() {
                        println!("Reading from: {}", key_file.display());
                        let key_content =
                            fs::read_to_string(key_file).context("reading AUR SSH key")?;

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

        SecretsCommand::Set {
            repo,
            name,
            value,
            from_file,
        } => {
            let repo_full = normalize_repo(&repo)?;

            let secret_value = if let Some(v) = value {
                v
            } else if let Some(path) = from_file {
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
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

    let release_cfg = &ctx.config.release;

    // Determine output directory
    let output_dir = cmd.output.clone().unwrap_or_else(|| PathBuf::from("."));
    let project_dir = output_dir.join(&cmd.name);

    if project_dir.exists() {
        return Err(anyhow!(
            "Directory '{}' already exists",
            project_dir.display()
        ));
    }

    // Branch based on whether we're using git clone or templates
    if let Some(ref git_source) = cmd.from_git {
        // Git-based scaffolding
        scaffold_from_git(ctx, &cmd, &project_dir, git_source)?;
    } else {
        // Template-based scaffolding (original behavior)
        scaffold_from_template(ctx, &cmd, &project_dir)?;
    }

    // Apply variable replacements unless skipped
    if !cmd.no_replace {
        println!("Configuring project...");
        replace_in_files(&project_dir, "{{project_name}}", &cmd.name)?;
        replace_in_files(&project_dir, "your-binary-name", &cmd.name)?;

        // For git-cloned repos, also replace common template patterns
        if cmd.from_git.is_some() {
            // Extract the repo name from the git URL for replacements
            if let Some(ref source) = cmd.from_git
                && let Some(source_name) = extract_repo_name(source)
            {
                replace_in_files(&project_dir, &source_name, &cmd.name)?;
                // Also try with underscores (for Python modules)
                let source_underscore = source_name.replace('-', "_");
                let name_underscore = cmd.name.replace('-', "_");
                if source_underscore != source_name {
                    replace_in_files(&project_dir, &source_underscore, &name_underscore)?;
                }
            }
        } else {
            // Rename directories/files if needed (e.g., python_cli -> project_name)
            rename_template_dirs(&project_dir, &cmd.template, &cmd.name)?;
        }
    }

    // Initialize git (only if not already a git repo from clone)
    if !project_dir.join(".git").exists() {
        println!("Initializing git...");
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(&project_dir)
            .output();
    }

    // Initialize beads if not already present
    if !project_dir.join(".beads").exists() {
        println!("Initializing beads...");
        let _ = Command::new("bd")
            .args(["init"])
            .current_dir(&project_dir)
            .output();
    }

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

/// Scaffold a project from a git repository
fn scaffold_from_git(
    ctx: &RuntimeContext,
    cmd: &NewCommand,
    project_dir: &Path,
    git_source: &str,
) -> Result<()> {
    use std::process::Command;

    // Normalize the git URL
    let git_url = normalize_git_url(git_source);

    println!("Creating new project from git: {}", cmd.name);
    println!("Source: {}", git_url);

    // Create temp dir for clone
    let temp_dir = std::env::temp_dir().join(format!("byt-git-{}", std::process::id()));
    fs::create_dir_all(&temp_dir)?;

    let clone_dir = temp_dir.join("repo");

    // Build git clone command
    let mut clone_args = vec!["clone", "--depth", "1"];

    // Add branch/tag/ref if specified
    if let Some(ref git_ref) = cmd.git_ref {
        clone_args.push("--branch");
        clone_args.push(git_ref);
    }

    clone_args.push(&git_url);
    clone_args.push(clone_dir.to_str().unwrap());

    println!("Cloning repository...");

    let output = Command::new("git")
        .args(&clone_args)
        .output()
        .context("cloning git repository")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(anyhow!("Failed to clone repository: {}", stderr.trim()));
    }

    // Determine source directory (either clone root or subdir)
    let source_dir = if let Some(ref subdir) = cmd.subdir {
        let subdir_path = clone_dir.join(subdir);
        if !subdir_path.exists() {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(anyhow!("Subdirectory '{}' not found in repository", subdir));
        }
        if !subdir_path.is_dir() {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(anyhow!("'{}' is not a directory", subdir));
        }
        subdir_path
    } else {
        clone_dir.clone()
    };

    println!("Creating project structure...");

    // Copy files to project directory, excluding .git if we're using a subdir
    // or if we want a fresh git history
    if cmd.subdir.is_some() {
        // When using subdir, always exclude .git (it's in the parent)
        copy_dir_filtered(&source_dir, project_dir, &[".git".to_string()])?;
    } else {
        // When cloning whole repo, remove .git to start fresh
        copy_dir_recursive(&source_dir, project_dir)?;
        let git_dir = project_dir.join(".git");
        if git_dir.exists() {
            fs::remove_dir_all(&git_dir)?;
        }
    }

    // Remove template.toml if present (byt-specific file)
    let template_toml = project_dir.join("template.toml");
    if template_toml.exists() {
        fs::remove_file(&template_toml)?;
    }

    // Clean up temp
    let _ = fs::remove_dir_all(&temp_dir);

    // If --dry-run, just show what would happen
    if ctx.common.dry_run {
        println!("dry-run: would create project at {}", project_dir.display());
    }

    Ok(())
}

/// Scaffold a project from the templates repository
fn scaffold_from_template(
    _ctx: &RuntimeContext,
    cmd: &NewCommand,
    project_dir: &Path,
) -> Result<()> {
    use std::process::Command;

    let templates_cfg = &_ctx.config.templates;

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
        .args([
            "-q",
            zip_path.to_str().unwrap(),
            "-d",
            extract_dir.to_str().unwrap(),
        ])
        .output()
        .context("extracting template")?;

    if !output.status.success() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(anyhow!("Failed to extract template"));
    }

    // Find the templates root directory (repo-branch/)
    let repo_name = templates_cfg
        .repo
        .split('/')
        .next_back()
        .unwrap_or("templates");
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
        let content = fs::read_to_string(&manifest_path).context("reading template.toml")?;
        Some(toml::from_str(&content).context("parsing template.toml")?)
    } else {
        None
    };

    // Copy template to project directory
    println!("Creating project structure...");
    copy_dir_recursive(&template_src, project_dir)?;

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
                eprintln!(
                    "Warning: Composed template '{}' not found, skipping",
                    comp.source
                );
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

    // Clean up temp
    let _ = fs::remove_dir_all(&temp_dir);

    Ok(())
}

/// Normalize a git source string to a full URL
/// Supports:
/// - Full URLs: https://github.com/user/repo, git@github.com:user/repo.git
/// - GitHub shorthand: user/repo
/// - GitLab shorthand: gitlab:user/repo
/// - Bitbucket shorthand: bitbucket:user/repo
fn normalize_git_url(source: &str) -> String {
    // Already a full URL
    if source.starts_with("https://")
        || source.starts_with("http://")
        || source.starts_with("git@")
        || source.starts_with("ssh://")
    {
        return source.to_string();
    }

    // Platform-specific shorthand
    if let Some(path) = source.strip_prefix("gitlab:") {
        return format!("https://gitlab.com/{}", path);
    }
    if let Some(path) = source.strip_prefix("bitbucket:") {
        return format!("https://bitbucket.org/{}", path);
    }
    if let Some(path) = source.strip_prefix("github:") {
        return format!("https://github.com/{}", path);
    }

    // Default: assume GitHub shorthand (user/repo)
    if source.contains('/') && !source.contains(':') {
        return format!("https://github.com/{}", source);
    }

    // Return as-is if we can't parse it
    source.to_string()
}

/// Extract repository name from a git URL or shorthand
fn extract_repo_name(source: &str) -> Option<String> {
    // Handle full URLs
    let path = if source.starts_with("https://") || source.starts_with("http://") {
        source.split('/').next_back()?
    } else if source.starts_with("git@") {
        // git@github.com:user/repo.git
        let parts: Vec<&str> = source.split(':').collect();
        if parts.len() >= 2 {
            parts[1].split('/').next_back()?
        } else {
            return None;
        }
    } else if source.contains('/') {
        // Shorthand like user/repo or github:user/repo
        let clean = source.split(':').next_back().unwrap_or(source);
        clean.split('/').next_back()?
    } else {
        return None;
    };

    // Remove .git suffix if present
    let name = path.strip_suffix(".git").unwrap_or(path);
    Some(name.to_string())
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
                if [
                    "png", "jpg", "jpeg", "gif", "ico", "woff", "woff2", "ttf", "lock",
                ]
                .contains(&ext.as_ref())
                {
                    continue;
                }
            }

            if let Ok(content) = fs::read_to_string(path)
                && content.contains(from)
            {
                let new_content = content.replace(from, to);
                fs::write(path, new_content)?;
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
    let catalog = ctx.load_catalog()?;

    let mut schemas = Vec::new();

    for (repo_name, repo_info) in &catalog.repos {
        let repo_path = workspace.join(&repo_info.path);

        // Check each pattern for schema files
        for pattern in &schema_cfg.patterns {
            let full_pattern = repo_path.join(pattern);
            if let Some(pattern_str) = full_pattern.to_str() {
                for source_path in glob(pattern_str)
                    .unwrap_or_else(|_| glob("").unwrap())
                    .flatten()
                {
                    // Determine destination path in schemas repo
                    let file_name = source_path
                        .file_name()
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
        let status = if schema.dest_path.exists() {
            "modified"
        } else {
            "new"
        };
        println!(
            "  [{}] {} -> {}",
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
        schemas
            .into_iter()
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

        let status = if schema.dest_path.exists() {
            "updated"
        } else {
            "added"
        };
        println!(
            "[{}] {} -> {}",
            status,
            schema.repo,
            schema.dest_path.display()
        );
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
            return Err(anyhow!(
                "git add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
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
            return Err(anyhow!(
                "git commit failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Git push
        let output = Command::new("git")
            .args(["push"])
            .current_dir(&schemas_repo)
            .output()
            .context("running git push")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git push failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
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
            if schema.dest_path.exists() {
                "needs update"
            } else {
                "new"
            }
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

// ============================================================================
// Website Sync
// ============================================================================

fn handle_website(ctx: &RuntimeContext, command: WebsiteCommand) -> Result<()> {
    match command {
        WebsiteCommand::Sync { commit, repos } => website_sync(ctx, commit, repos),
        WebsiteCommand::List => website_list(ctx),
        WebsiteCommand::Check => website_check(ctx),
    }
}

/// Metadata from a tool.toml file
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolToml {
    name: String,
    title: String,
    tagline: String,
    #[serde(default)]
    tagline_de: Option<String>,
    description: String,
    #[serde(default)]
    description_de: Option<String>,
    language: String,
    category: String,
    version: String,
    license: String,
    #[serde(default)]
    install: ToolInstall,
    #[serde(default)]
    links: ToolLinks,
    #[serde(default)]
    features: Vec<ToolFeature>,
    #[serde(default)]
    examples: Vec<ToolExample>,
    #[serde(default)]
    media: ToolMedia,
    #[serde(default)]
    meta: ToolMeta,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ToolInstall {
    #[serde(default)]
    homebrew: Option<String>,
    #[serde(default)]
    aur: Option<String>,
    #[serde(default)]
    aur_cuda: Option<String>,
    #[serde(default)]
    cargo: Option<String>,
    #[serde(default)]
    go: Option<String>,
    #[serde(default)]
    npm: Option<String>,
    #[serde(default)]
    pip: Option<String>,
    #[serde(default)]
    binary: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ToolLinks {
    #[serde(default)]
    github: Option<String>,
    #[serde(default)]
    docs: Option<String>,
    #[serde(default)]
    changelog: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolFeature {
    title: String,
    #[serde(default)]
    title_de: Option<String>,
    description: String,
    #[serde(default)]
    description_de: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolExample {
    title: String,
    language: String,
    code: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ToolMedia {
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    logo: Option<String>,
    #[serde(default)]
    screenshot: Option<String>,
    #[serde(default)]
    video: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ToolMeta {
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    platforms: Vec<String>,
    #[serde(default)]
    related: Vec<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    listed: Option<bool>,
}

/// Found tool.toml with metadata
#[derive(Debug)]
struct FoundTool {
    repo_name: String,
    repo_path: PathBuf,
    tool: ToolToml,
}

fn find_tools(ctx: &RuntimeContext) -> Result<Vec<FoundTool>> {
    let workspace = ctx.workspace_root();
    let mut tools = Vec::new();

    // Scan workspace for repos with tool.toml
    for entry in fs::read_dir(&workspace)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        // Skip ignored directories
        let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if ctx.config.ignore_dirs.contains(&dir_name.to_string()) {
            continue;
        }

        let tool_toml_path = path.join("tool.toml");
        if tool_toml_path.exists() {
            let content = fs::read_to_string(&tool_toml_path)
                .with_context(|| format!("reading {}", tool_toml_path.display()))?;

            let tool: ToolToml = toml::from_str(&content)
                .with_context(|| format!("parsing {}", tool_toml_path.display()))?;

            // Only include if listed != false
            if tool.meta.listed.unwrap_or(true) {
                tools.push(FoundTool {
                    repo_name: dir_name.to_string(),
                    repo_path: path.clone(),
                    tool,
                });
            }
        }
    }

    // Sort by name
    tools.sort_by(|a, b| a.tool.name.cmp(&b.tool.name));

    Ok(tools)
}

fn website_list(ctx: &RuntimeContext) -> Result<()> {
    let tools = find_tools(ctx)?;

    if tools.is_empty() {
        println!("No tool.toml files found in workspace.");
        return Ok(());
    }

    println!("Tools with tool.toml:\n");
    for t in &tools {
        let status = t.tool.meta.status.as_deref().unwrap_or("stable");
        println!("  {} [{}] - {}", t.tool.name, status, t.tool.tagline);
        println!("    repo: {}", t.repo_name);
        println!("    version: {}", t.tool.version);
        println!();
    }

    Ok(())
}

fn website_check(ctx: &RuntimeContext) -> Result<()> {
    let tools = find_tools(ctx)?;
    let website_cfg = &ctx.config.website;

    let workspace = ctx.workspace_root();
    let website_repo = if PathBuf::from(&website_cfg.repo_path).is_absolute() {
        PathBuf::from(&website_cfg.repo_path)
    } else {
        workspace.join(&website_cfg.repo_path)
    };

    if !website_repo.exists() {
        return Err(anyhow!(
            "Website repo not found at {}. Clone it first or update website.repo_path in config.",
            website_repo.display()
        ));
    }

    let public_dir = website_repo.join(&website_cfg.public_dir);

    println!("Checking tool data files:\n");
    let mut needs_update = 0;

    for t in &tools {
        let data_path = public_dir.join(&t.tool.name).join("data.json");
        let status = if data_path.exists() {
            "exists"
        } else {
            needs_update += 1;
            "missing"
        };
        println!("  {} [{}]", t.tool.name, status);
    }

    if needs_update > 0 {
        println!(
            "\n{} tool(s) need data files. Run 'byt website sync' to generate.",
            needs_update
        );
    } else {
        println!("\nAll tools have data files.");
    }

    Ok(())
}

fn website_sync(ctx: &RuntimeContext, commit: bool, filter_repos: Vec<String>) -> Result<()> {
    use std::process::Command;

    let tools = find_tools(ctx)?;
    let website_cfg = &ctx.config.website;

    // Filter tools
    let tools: Vec<_> = if filter_repos.is_empty() {
        tools
    } else {
        tools
            .into_iter()
            .filter(|t| filter_repos.contains(&t.repo_name) || filter_repos.contains(&t.tool.name))
            .collect()
    };

    if tools.is_empty() {
        println!("No tools to sync.");
        return Ok(());
    }

    let workspace = ctx.workspace_root();
    let website_repo = if PathBuf::from(&website_cfg.repo_path).is_absolute() {
        PathBuf::from(&website_cfg.repo_path)
    } else {
        workspace.join(&website_cfg.repo_path)
    };

    if !website_repo.exists() {
        return Err(anyhow!(
            "Website repo not found at {}. Clone it first or update website.repo_path in config.",
            website_repo.display()
        ));
    }

    let public_dir = website_repo.join(&website_cfg.public_dir);

    // Create directories if needed
    fs::create_dir_all(&public_dir)?;

    // Generate tool data JSON files (consumed by website .tsx pages)
    for t in &tools {
        let tool_data = generate_tool_data_json(&t.tool)?;
        let tool_public_dir = public_dir.join(&t.tool.name);
        fs::create_dir_all(&tool_public_dir)?;
        let data_path = tool_public_dir.join("data.json");

        // Check if content changed
        let needs_write = if data_path.exists() {
            let existing = fs::read_to_string(&data_path)?;
            existing != tool_data
        } else {
            true
        };

        if needs_write {
            fs::write(&data_path, &tool_data)?;
            println!("[updated] {}", t.tool.name);
        } else {
            println!("[unchanged] {}", t.tool.name);
        }

        // Copy media files if they exist
        if let Some(ref screenshot) = t.tool.media.screenshot {
            // Only copy if it's a local path (not a URL)
            if !screenshot.starts_with("http") {
                let src = t.repo_path.join(screenshot);
                if src.exists() {
                    let filename = src.file_name().unwrap_or_default();
                    let dest = tool_public_dir.join(filename);
                    fs::copy(&src, &dest)?;
                    println!("  copied screenshot: {}", filename.to_string_lossy());
                }
            }
        }

        if let Some(ref icon) = t.tool.media.icon {
            if !icon.starts_with("http") {
                let src = t.repo_path.join(icon);
                if src.exists() {
                    let filename = src.file_name().unwrap_or_default();
                    let dest = tool_public_dir.join(filename);
                    fs::copy(&src, &dest)?;
                    println!("  copied icon: {}", filename.to_string_lossy());
                }
            }
        }

        if let Some(ref logo) = t.tool.media.logo {
            if !logo.starts_with("http") {
                let src = t.repo_path.join(logo);
                if src.exists() {
                    let filename = src.file_name().unwrap_or_default();
                    let dest = tool_public_dir.join(filename);
                    fs::copy(&src, &dest)?;
                    println!("  copied logo: {}", filename.to_string_lossy());
                }
            }
        }
    }

    // Generate tools index JSON (consumed by website index.tsx)
    let tools_index = generate_tools_index_json(&tools)?;
    let tools_json_path = public_dir.join("tools.json");
    fs::write(&tools_json_path, &tools_index)?;
    println!("[updated] public/toolz/tools.json");

    // Generate markdown versions of tool pages for LLM consumption
    let toolz_md_dir = website_repo.join("public").join("toolz");
    fs::create_dir_all(&toolz_md_dir)?;
    for t in &tools {
        let md_content = generate_tool_markdown(&t.tool);
        let md_path = toolz_md_dir.join(format!("{}.md", t.tool.name));
        fs::write(&md_path, &md_content)?;
        println!("[updated] public/toolz/{}.md", t.tool.name);
    }

    // Generate toolz index markdown
    let toolz_index_md = generate_toolz_index_markdown(&tools);
    let toolz_index_path = toolz_md_dir.join("index.md");
    fs::write(&toolz_index_path, &toolz_index_md)?;
    println!("[updated] public/toolz/index.md");

    // Generate about page markdown
    let about_md = generate_about_markdown();
    let about_path = website_repo.join("public").join("about.md");
    fs::write(&about_path, &about_md)?;
    println!("[updated] public/about.md");

    // Generate llms.txt (after all md files are created)
    let llms_txt = generate_llms_txt(&tools, "https://byteowlz.com");
    let llms_path = website_repo.join("public").join("llms.txt");
    fs::write(&llms_path, &llms_txt)?;
    println!("[updated] public/llms.txt");

    if commit {
        println!("\nCommitting changes...");

        // Git add
        let output = Command::new("git")
            .args(["add", "."])
            .current_dir(&website_repo)
            .output()
            .context("running git add")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Check if there are changes to commit
        let output = Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(&website_repo)
            .output()?;

        if output.status.success() {
            println!("No changes to commit.");
            return Ok(());
        }

        // Generate commit message
        let tool_names: Vec<_> = tools.iter().map(|t| t.tool.name.as_str()).collect();
        let commit_msg = if tool_names.len() == 1 {
            format!("Update {} tool page", tool_names[0])
        } else {
            format!("Update tool pages: {}", tool_names.join(", "))
        };

        // Git commit
        let output = Command::new("git")
            .args(["commit", "-m", &commit_msg])
            .current_dir(&website_repo)
            .output()
            .context("running git commit")?;

        if !output.status.success() {
            return Err(anyhow!(
                "git commit failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        println!("Changes committed.");
    } else {
        println!("\nPages synced. Run 'byt website sync --commit' to commit changes.");
    }

    Ok(())
}

/// Generate a JSON data file for a single tool (consumed by website .tsx pages)
fn generate_tool_data_json(tool: &ToolToml) -> Result<String> {
    let screenshot_url = tool.media.screenshot.as_ref().map(|s| {
        if s.starts_with("http") {
            s.clone()
        } else {
            format!("/toolz/{}/{}", tool.name, s.split('/').last().unwrap_or(s))
        }
    });

    let logo_url = tool.media.logo.as_ref().map(|s| {
        if s.starts_with("http") {
            s.clone()
        } else {
            format!("/toolz/{}/{}", tool.name, s.split('/').last().unwrap_or(s))
        }
    });

    let status = tool.meta.status.as_deref().unwrap_or("stable");

    let data = serde_json::json!({
        "name": tool.name,
        "title": tool.title,
        "tagline": tool.tagline,
        "tagline_de": tool.tagline_de,
        "description": tool.description,
        "description_de": tool.description_de,
        "language": tool.language,
        "category": tool.category,
        "version": tool.version,
        "license": tool.license,
        "install": {
            "homebrew": tool.install.homebrew,
            "aur": tool.install.aur,
            "aur_cuda": tool.install.aur_cuda,
            "cargo": tool.install.cargo,
            "go": tool.install.go,
            "npm": tool.install.npm,
            "pip": tool.install.pip,
            "binary": tool.install.binary,
        },
        "links": {
            "github": tool.links.github,
            "docs": tool.links.docs,
            "changelog": tool.links.changelog,
        },
        "features": tool.features.iter().map(|f| serde_json::json!({
            "title": f.title,
            "title_de": f.title_de,
            "description": f.description,
            "description_de": f.description_de,
        })).collect::<Vec<_>>(),
        "examples": tool.examples,
        "media": {
            "screenshot": screenshot_url,
            "logo": logo_url,
            "icon": tool.media.icon,
            "video": tool.media.video,
        },
        "meta": {
            "keywords": tool.meta.keywords,
            "platforms": tool.meta.platforms,
            "related": tool.meta.related,
            "status": status,
        },
    });

    Ok(serde_json::to_string_pretty(&data)?)
}

/// Generate a JSON index of all tools (consumed by website index.tsx)
fn generate_tools_index_json(tools: &[FoundTool]) -> Result<String> {
    let tools_json: Vec<_> = tools
        .iter()
        .map(|t| {
            let logo_url = t.tool.media.logo.as_ref().map(|s| {
                if s.starts_with("http") {
                    s.clone()
                } else {
                    format!("/toolz/{}/{}", t.tool.name, s.split('/').last().unwrap_or(s))
                }
            });
            serde_json::json!({
                "name": t.tool.name,
                "title": t.tool.title,
                "tagline": t.tool.tagline,
                "tagline_de": t.tool.tagline_de,
                "category": t.tool.category,
                "language": t.tool.language,
                "status": t.tool.meta.status.as_deref().unwrap_or("stable"),
                "logo": logo_url,
            })
        })
        .collect();

    Ok(serde_json::to_string_pretty(&tools_json)?)
}

fn generate_llms_txt(tools: &[FoundTool], site_url: &str) -> String {
    let mut content = String::new();

    // Header
    content.push_str("# byteowlz\n\n");
    content.push_str(
        "> Open source tools for humans and AI agents. Local-first, cross-platform, CLI-first.\n\n",
    );
    content.push_str("byteowlz builds developer tools that run locally, work across platforms, and integrate seamlessly with AI workflows. No cloud dependencies, no subscriptions - your data stays yours.\n\n");

    // Tools section
    if !tools.is_empty() {
        content.push_str("## Tools\n\n");
        content.push_str(&format!(
            "- [All Tools]({}/toolz/index.md): Overview of all byteowlz tools\n",
            site_url
        ));
        for t in tools {
            // Link to markdown version of tool page for LLM consumption
            content.push_str(&format!(
                "- [{}]({}/toolz/{}.md): {}\n",
                t.tool.name, site_url, t.tool.name, t.tool.tagline
            ));
        }
        content.push('\n');
    }

    // Optional section
    content.push_str("## Optional\n\n");
    content.push_str("- [GitHub](https://github.com/byteowlz): All our open source projects\n");
    content.push_str(&format!(
        "- [About]({}/about.md): About byteowlz\n",
        site_url
    ));

    content
}

fn generate_toolz_index_markdown(tools: &[FoundTool]) -> String {
    let mut content = String::new();

    content.push_str("# byteowlz Tools\n\n");
    content.push_str("Open source tools built by byteowlz. Local-first, cross-platform, CLI-first. No cloud dependencies, no subscriptions - your data stays yours.\n\n");

    if tools.is_empty() {
        content.push_str("*No tools available yet. Check back soon!*\n");
        return content;
    }

    content.push_str("## Available Tools\n\n");
    content.push_str("| Tool | Description | Language | Status |\n");
    content.push_str("|------|-------------|----------|--------|\n");

    for t in tools {
        let status = t.tool.meta.status.as_deref().unwrap_or("stable");
        content.push_str(&format!(
            "| [{}]({}.md) | {} | {} | {} |\n",
            t.tool.name, t.tool.name, t.tool.tagline, t.tool.language, status
        ));
    }

    content.push_str("\n---\n\n");
    content.push_str(
        "For more information, visit [github.com/byteowlz](https://github.com/byteowlz)\n",
    );

    content
}

fn generate_about_markdown() -> String {
    let mut content = String::new();

    content.push_str("# About byteowlz\n\n");
    content.push_str(
        "byteowlz is a software company focused on building open source developer tools.\n\n",
    );
    content.push_str("## Philosophy\n\n");
    content.push_str("- **Local-First**: Your data stays on your machine. No cloud dependencies, no subscriptions.\n");
    content.push_str("- **Cross-Platform**: Linux, macOS, Windows. First-class support for all major platforms.\n");
    content
        .push_str("- **CLI-First**: Designed for terminal workflows and AI agent integration.\n");
    content.push_str(
        "- **Open Source**: All our core tools are open source under permissive licenses.\n\n",
    );
    content.push_str("## Contact\n\n");
    content.push_str("- GitHub: [github.com/byteowlz](https://github.com/byteowlz)\n");
    content.push_str("- Website: [byteowlz.com](https://byteowlz.com)\n");

    content
}

fn generate_tool_markdown(tool: &ToolToml) -> String {
    let mut content = String::new();

    // Header
    content.push_str(&format!("# {}\n\n", tool.name));
    content.push_str(&format!("*{}*\n\n", tool.tagline));

    // Metadata
    content.push_str(&format!(
        "**Version:** {} | **License:** {} | **Language:** {}\n\n",
        tool.version, tool.license, tool.language
    ));

    if let Some(ref github) = tool.links.github {
        content.push_str(&format!("**GitHub:** {}\n\n", github));
    }

    // Description
    content.push_str("## Description\n\n");
    content.push_str(&tool.description);
    content.push_str("\n\n");

    // Installation
    content.push_str("## Installation\n\n");
    if let Some(ref v) = tool.install.homebrew {
        content.push_str(&format!(
            "**Homebrew:**\n```bash\nbrew install {}\n```\n\n",
            v
        ));
    }
    if let Some(ref v) = tool.install.aur {
        content.push_str(&format!("**AUR:**\n```bash\nyay -S {}\n```\n\n", v));
    }
    if let Some(ref v) = tool.install.cargo {
        content.push_str(&format!(
            "**Cargo:**\n```bash\ncargo install {}\n```\n\n",
            v
        ));
    }
    if let Some(ref v) = tool.install.pip {
        content.push_str(&format!("**pip:**\n```bash\npip install {}\n```\n\n", v));
    }
    if let Some(ref v) = tool.install.npm {
        content.push_str(&format!("**npm:**\n```bash\nnpm install -g {}\n```\n\n", v));
    }
    if let Some(ref v) = tool.install.go {
        content.push_str(&format!("**Go:**\n```bash\ngo install {}\n```\n\n", v));
    }

    // Features
    if !tool.features.is_empty() {
        content.push_str("## Features\n\n");
        for f in &tool.features {
            content.push_str(&format!("- **{}**: {}\n", f.title, f.description));
        }
        content.push_str("\n");
    }

    // Examples
    if !tool.examples.is_empty() {
        content.push_str("## Usage Examples\n\n");
        for ex in &tool.examples {
            content.push_str(&format!("### {}\n\n", ex.title));
            let lang = if ex.language.is_empty() {
                "bash"
            } else {
                &ex.language
            };
            content.push_str(&format!("```{}\n{}\n```\n\n", lang, ex.code));
        }
    }

    // Platforms
    if !tool.meta.platforms.is_empty() {
        content.push_str("## Platforms\n\n");
        content.push_str(&format!("{}\n\n", tool.meta.platforms.join(", ")));
    }

    // Keywords
    if !tool.meta.keywords.is_empty() {
        content.push_str("---\n\n");
        content.push_str(&format!("*Keywords: {}*\n", tool.meta.keywords.join(", ")));
    }

    content
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
    // Find the byteowlz workspace root.
    // The workspace root contains multiple repos including govnr/.
    // We look for: govnr/CATALOG.json (primary marker of workspace root)
    let current = env::current_dir()?;

    // Walk up looking for the workspace root
    let mut dir = current.as_path();
    loop {
        // Primary check: workspace root has govnr/ subdirectory with CATALOG.json
        let govnr_catalog = dir.join("govnr").join("CATALOG.json");
        if govnr_catalog.exists() {
            return Ok(dir.to_path_buf());
        }

        // Secondary check: we might be inside govnr/, so check if parent has govnr/CATALOG.json
        if let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) {
            if dir_name == "govnr" {
                let catalog = dir.join("CATALOG.json");
                if catalog.exists() {
                    // We're in the govnr dir, parent is the workspace root
                    if let Some(parent) = dir.parent() {
                        return Ok(parent.to_path_buf());
                    }
                }
            }
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Git URL normalization tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_normalize_git_url_full_https() {
        let url = "https://github.com/user/repo";
        assert_eq!(normalize_git_url(url), url);
    }

    #[test]
    fn test_normalize_git_url_full_http() {
        let url = "http://github.com/user/repo";
        assert_eq!(normalize_git_url(url), url);
    }

    #[test]
    fn test_normalize_git_url_git_ssh() {
        let url = "git@github.com:user/repo.git";
        assert_eq!(normalize_git_url(url), url);
    }

    #[test]
    fn test_normalize_git_url_ssh_protocol() {
        let url = "ssh://git@github.com/user/repo";
        assert_eq!(normalize_git_url(url), url);
    }

    #[test]
    fn test_normalize_git_url_github_shorthand() {
        assert_eq!(
            normalize_git_url("user/repo"),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn test_normalize_git_url_github_prefix() {
        assert_eq!(
            normalize_git_url("github:user/repo"),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn test_normalize_git_url_gitlab_prefix() {
        assert_eq!(
            normalize_git_url("gitlab:user/repo"),
            "https://gitlab.com/user/repo"
        );
    }

    #[test]
    fn test_normalize_git_url_bitbucket_prefix() {
        assert_eq!(
            normalize_git_url("bitbucket:user/repo"),
            "https://bitbucket.org/user/repo"
        );
    }

    #[test]
    fn test_normalize_git_url_unknown_format() {
        let url = "some-random-string";
        assert_eq!(normalize_git_url(url), url);
    }

    // -------------------------------------------------------------------------
    // Repository name extraction tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_extract_repo_name_https_url() {
        assert_eq!(
            extract_repo_name("https://github.com/user/myrepo"),
            Some("myrepo".to_string())
        );
    }

    #[test]
    fn test_extract_repo_name_https_with_git_suffix() {
        assert_eq!(
            extract_repo_name("https://github.com/user/myrepo.git"),
            Some("myrepo".to_string())
        );
    }

    #[test]
    fn test_extract_repo_name_ssh_url() {
        assert_eq!(
            extract_repo_name("git@github.com:user/myrepo.git"),
            Some("myrepo".to_string())
        );
    }

    #[test]
    fn test_extract_repo_name_shorthand() {
        assert_eq!(extract_repo_name("user/myrepo"), Some("myrepo".to_string()));
    }

    #[test]
    fn test_extract_repo_name_github_prefix() {
        assert_eq!(
            extract_repo_name("github:user/myrepo"),
            Some("myrepo".to_string())
        );
    }

    #[test]
    fn test_extract_repo_name_nested_path() {
        assert_eq!(
            extract_repo_name("https://github.com/org/group/myrepo"),
            Some("myrepo".to_string())
        );
    }

    #[test]
    fn test_extract_repo_name_invalid() {
        assert_eq!(extract_repo_name("single-word"), None);
    }

    // -------------------------------------------------------------------------
    // Copy directory tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_copy_dir_recursive() {
        let temp = std::env::temp_dir().join("byt-test-copy");
        let src = temp.join("src");
        let dst = temp.join("dst");

        // Clean up from previous runs
        let _ = fs::remove_dir_all(&temp);

        // Create source structure
        fs::create_dir_all(src.join("subdir")).unwrap();
        fs::write(src.join("file1.txt"), "content1").unwrap();
        fs::write(src.join("subdir/file2.txt"), "content2").unwrap();

        // Copy
        copy_dir_recursive(&src, &dst).unwrap();

        // Verify
        assert!(dst.join("file1.txt").exists());
        assert!(dst.join("subdir/file2.txt").exists());
        assert_eq!(
            fs::read_to_string(dst.join("file1.txt")).unwrap(),
            "content1"
        );
        assert_eq!(
            fs::read_to_string(dst.join("subdir/file2.txt")).unwrap(),
            "content2"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_copy_dir_filtered() {
        let temp = std::env::temp_dir().join("byt-test-filter");
        let src = temp.join("src");
        let dst = temp.join("dst");

        // Clean up from previous runs
        let _ = fs::remove_dir_all(&temp);

        // Create source structure
        fs::create_dir_all(src.join(".git")).unwrap();
        fs::write(src.join("keep.txt"), "keep").unwrap();
        fs::write(src.join(".git/config"), "git config").unwrap();
        fs::write(src.join("skip.lock"), "lock file").unwrap();

        // Copy with exclusions
        copy_dir_filtered(&src, &dst, &[".git".to_string(), "*.lock".to_string()]).unwrap();

        // Verify
        assert!(dst.join("keep.txt").exists());
        assert!(!dst.join(".git").exists());
        assert!(!dst.join("skip.lock").exists());

        // Cleanup
        let _ = fs::remove_dir_all(&temp);
    }

    // -------------------------------------------------------------------------
    // Replace in files tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_replace_in_files() {
        let temp = std::env::temp_dir().join("byt-test-replace");

        // Clean up from previous runs
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();

        // Create test file
        fs::write(temp.join("test.txt"), "Hello {{project_name}}!").unwrap();

        // Replace
        replace_in_files(&temp, "{{project_name}}", "myproject").unwrap();

        // Verify
        assert_eq!(
            fs::read_to_string(temp.join("test.txt")).unwrap(),
            "Hello myproject!"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_replace_in_files_skips_binary() {
        let temp = std::env::temp_dir().join("byt-test-binary");

        // Clean up from previous runs
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();

        // Create a "binary" file (by extension)
        fs::write(temp.join("image.png"), "fake png {{project_name}}").unwrap();
        fs::write(temp.join("Cargo.lock"), "lock {{project_name}}").unwrap();

        // Replace should skip these files
        replace_in_files(&temp, "{{project_name}}", "myproject").unwrap();

        // Verify they are unchanged
        assert!(
            fs::read_to_string(temp.join("image.png"))
                .unwrap()
                .contains("{{project_name}}")
        );
        assert!(
            fs::read_to_string(temp.join("Cargo.lock"))
                .unwrap()
                .contains("{{project_name}}")
        );

        // Cleanup
        let _ = fs::remove_dir_all(&temp);
    }

    // -------------------------------------------------------------------------
    // NewCommand argument tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_new_command_default_template() {
        use clap::Parser;

        let args = vec!["byt", "new", "myproject"];
        let cli = Cli::try_parse_from(args).unwrap();

        if let Command::New(cmd) = cli.command {
            assert_eq!(cmd.name, "myproject");
            assert_eq!(cmd.template, "rust-cli");
            assert!(cmd.from_git.is_none());
            assert!(cmd.subdir.is_none());
            assert!(!cmd.no_replace);
        } else {
            panic!("Expected New command");
        }
    }

    #[test]
    fn test_new_command_from_git() {
        use clap::Parser;

        let args = vec!["byt", "new", "myproject", "--from-git", "user/repo"];
        let cli = Cli::try_parse_from(args).unwrap();

        if let Command::New(cmd) = cli.command {
            assert_eq!(cmd.name, "myproject");
            assert_eq!(cmd.from_git, Some("user/repo".to_string()));
        } else {
            panic!("Expected New command");
        }
    }

    #[test]
    fn test_new_command_from_git_with_subdir() {
        use clap::Parser;

        let args = vec![
            "byt",
            "new",
            "myproject",
            "--from-git",
            "user/repo",
            "--subdir",
            "templates/rust",
        ];
        let cli = Cli::try_parse_from(args).unwrap();

        if let Command::New(cmd) = cli.command {
            assert_eq!(cmd.from_git, Some("user/repo".to_string()));
            assert_eq!(cmd.subdir, Some("templates/rust".to_string()));
        } else {
            panic!("Expected New command");
        }
    }

    #[test]
    fn test_new_command_from_git_with_ref() {
        use clap::Parser;

        let args = vec![
            "byt",
            "new",
            "myproject",
            "--from-git",
            "user/repo",
            "--git-ref",
            "v1.0.0",
        ];
        let cli = Cli::try_parse_from(args).unwrap();

        if let Command::New(cmd) = cli.command {
            assert_eq!(cmd.from_git, Some("user/repo".to_string()));
            assert_eq!(cmd.git_ref, Some("v1.0.0".to_string()));
        } else {
            panic!("Expected New command");
        }
    }

    #[test]
    fn test_new_command_no_replace() {
        use clap::Parser;

        let args = vec![
            "byt",
            "new",
            "myproject",
            "--from-git",
            "user/repo",
            "--no-replace",
        ];
        let cli = Cli::try_parse_from(args).unwrap();

        if let Command::New(cmd) = cli.command {
            assert!(cmd.no_replace);
        } else {
            panic!("Expected New command");
        }
    }

    #[test]
    fn test_new_command_subdir_requires_from_git() {
        use clap::Parser;

        // --subdir without --from-git should fail
        let args = vec!["byt", "new", "myproject", "--subdir", "templates/rust"];
        let result = Cli::try_parse_from(args);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_command_git_ref_requires_from_git() {
        use clap::Parser;

        // --git-ref without --from-git should fail
        let args = vec!["byt", "new", "myproject", "--git-ref", "v1.0.0"];
        let result = Cli::try_parse_from(args);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_command_from_git_conflicts_with_template() {
        use clap::Parser;

        // --from-git with -t should fail (they conflict)
        let args = vec![
            "byt",
            "new",
            "myproject",
            "--from-git",
            "user/repo",
            "-t",
            "python-cli",
        ];
        let result = Cli::try_parse_from(args);
        assert!(result.is_err());
    }
}
