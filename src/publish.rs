//! `byt publish`: export a private working repository into its public mirror.
//!
//! The working repository (private, e.g. on Forgejo) holds everything: agent
//! instructions, decision records, research. Its public mirror receives only
//! committed content, with markdown limited to an allowlist (README, changelog
//! and actual documentation), agent/context files never, and nothing that
//! matches the leak scan. The export lands as one snapshot commit in a
//! separate checkout (`~/byteowlz-public/<repo>` by default) and is pushed
//! only with `--push`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use glob::{MatchOptions, Pattern};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::RuntimeContext;

const CONFIG_FILE: &str = "publish.toml";
const PUBLIC_ROOT: &str = "~/byteowlz-public";

/// Never published, whatever the configuration says.
const ALWAYS_EXCLUDED_DIRS: [&str; 6] = [".trx", ".pi", ".claude", ".auto", ".sync", ".internal"];
const AGENT_FILES: [&str; 5] = [
    "AGENTS.md",
    "CLAUDE.md",
    "CLAUDE.local.md",
    "GEMINI.md",
    "CONTEXT.md",
];
const MARKDOWN: [&str; 3] = ["md", "markdown", "mdx"];
/// Placeholder user names that are not a real home directory.
const BENIGN_USERS: [&str; 12] = [
    "user", "username", "me", "you", "runner", "u", "example", "alice", "bob", "name", "someone",
    "dev",
];

#[derive(Debug, Subcommand)]
pub enum PublishCommand {
    /// Show what an export would change and run the leak scan; writes nothing
    Check {
        /// Repository to export (default: the current one)
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Export into the public checkout as one snapshot commit
    Export {
        /// Repository to export (default: the current one)
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Push the public checkout after committing
        #[arg(long)]
        push: bool,
        /// With --push: delete every public branch other than main
        #[arg(long, requires = "push")]
        prune_branches: bool,
        /// Tag the snapshot for a release. The private HEAD must carry the same
        /// tag; without a value, the tag at the private HEAD is used.
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        tag: Option<String>,
    },
    /// Write a starter publish.toml listing this repository's markdown
    Init {
        /// Repository to configure (default: the current one)
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishConfig {
    /// Public checkout; default `~/byteowlz-public/<repository directory name>`.
    public_dir: Option<String>,
    /// Cloned into `public_dir` when it does not exist yet.
    public_remote: Option<String>,
    /// Markdown allowed in the export (globs relative to the root).
    #[serde(default)]
    docs: Vec<String>,
    /// Further tracked paths left out (globs; a directory excludes its contents).
    #[serde(default)]
    exclude: Vec<String>,
    /// Case-insensitive terms that block the export (third-party names, hosts).
    #[serde(default)]
    forbidden: Vec<String>,
    /// Exact leak-scan matches that are known to be harmless.
    #[serde(default)]
    allow: Vec<String>,
    /// Also run `nym detect` over the export (fails if nym is not installed).
    #[serde(default)]
    nym: bool,
    /// Extra arguments for `nym detect`, e.g. `["--ruleset", "secrets"]`.
    #[serde(default)]
    nym_args: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Leak {
    path: String,
    line: usize,
    rule: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct Report {
    repo: String,
    head: String,
    public_dir: String,
    files: usize,
    dropped_markdown: Vec<String>,
    excluded: Vec<String>,
    added: Vec<String>,
    removed: Vec<String>,
    changed: Vec<String>,
    leaks: Vec<Leak>,
    /// Public branches other than main; the mirror publishes main only.
    extra_branches: Vec<String>,
    pruned_branches: Vec<String>,
    committed: Option<String>,
    /// Release tag created on the snapshot.
    tagged: Option<String>,
    pushed: bool,
}

pub fn handle_publish(ctx: &RuntimeContext, command: PublishCommand) -> Result<()> {
    match command {
        PublishCommand::Init { repo } => init(&repo_root(repo)?),
        PublishCommand::Check { repo } => {
            let report = run(&repo_root(repo)?, false, false, false, None)?;
            finish(ctx, &report)
        }
        PublishCommand::Export {
            repo,
            push,
            prune_branches,
            tag,
        } => {
            let apply = !ctx.common.dry_run;
            let root = repo_root(repo)?;
            let tag = tag.map(|t| release_tag(&root, &t)).transpose()?;
            let report = run(&root, apply, push && apply, prune_branches && apply, tag)?;
            finish(ctx, &report)
        }
    }
}

fn finish(ctx: &RuntimeContext, report: &Report) -> Result<()> {
    if ctx.common.json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print_report(report);
    }
    if !report.leaks.is_empty() {
        bail!(
            "{} problem(s) block the export; fix them, or add exact leak-scan matches to `allow` in {CONFIG_FILE}",
            report.leaks.len()
        );
    }
    Ok(())
}

fn print_report(r: &Report) {
    println!("{} @ {} -> {}", r.repo, r.head, r.public_dir);
    println!(
        "{} files; {} added, {} removed, {} changed; {} markdown files and {} paths kept private",
        r.files,
        r.added.len(),
        r.removed.len(),
        r.changed.len(),
        r.dropped_markdown.len(),
        r.excluded.len()
    );
    for (label, list) in [
        ("added", &r.added),
        ("removed", &r.removed),
        ("changed", &r.changed),
    ] {
        for path in list {
            println!("  {label:8} {path}");
        }
    }
    for branch in &r.extra_branches {
        if r.pruned_branches.contains(branch) {
            println!("  BRANCH {branch}: deleted");
        } else {
            println!(
                "  BRANCH {branch}: only main belongs on the public side; delete with --push --prune-branches"
            );
        }
    }
    for leak in &r.leaks {
        println!(
            "  LEAK {}:{} [{}] {}",
            leak.path, leak.line, leak.rule, leak.text
        );
    }
    match (&r.committed, r.pushed) {
        (Some(c), true) => println!("committed {c} and pushed"),
        (Some(c), false) => println!(
            "committed {c} (not pushed; review, then `git -C {} push`)",
            r.public_dir
        ),
        (None, _) => {}
    }
}

fn repo_root(repo: Option<PathBuf>) -> Result<PathBuf> {
    let start = match repo {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    let top =
        git(&start, &["rev-parse", "--show-toplevel"]).context("not inside a git repository")?;
    Ok(PathBuf::from(top))
}

fn load_config(root: &Path) -> Result<PublishConfig> {
    let path = root.join(CONFIG_FILE);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("{} missing; run `byt publish init` first", path.display()))?;
    let cfg: PublishConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    for doc in &cfg.docs {
        let name = Path::new(doc)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if AGENT_FILES.contains(&name) {
            bail!("{CONFIG_FILE}: `{doc}` is an agent/context file and is never published");
        }
    }
    Ok(cfg)
}

fn public_dir(root: &Path, cfg: &PublishConfig) -> Result<PathBuf> {
    let raw = match &cfg.public_dir {
        Some(dir) => dir.clone(),
        None => {
            let name = root
                .file_name()
                .and_then(|n| n.to_str())
                .context("repository has no directory name")?;
            format!("{PUBLIC_ROOT}/{name}")
        }
    };
    let dir = PathBuf::from(shellexpand::tilde(&raw).into_owned());
    if dir.starts_with(root) || root.starts_with(&dir) {
        bail!(
            "public_dir {} overlaps the working repository",
            dir.display()
        );
    }
    Ok(dir)
}

/// The public mirror carries one branch.
const PUBLIC_BRANCH: &str = "main";

fn remote_branches(public: &Path) -> Vec<String> {
    git(public, &["ls-remote", "--heads", "origin"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter_map(|r| r.strip_prefix("refs/heads/"))
        .filter(|b| *b != PUBLIC_BRANCH)
        .map(str::to_owned)
        .collect()
}

/// The release tag to put on the snapshot: it must point at the private HEAD,
/// so a public release always matches a tagged private commit.
fn release_tag(root: &Path, requested: &str) -> Result<String> {
    let at_head = git(root, &["tag", "--points-at", "HEAD"])?;
    let tags: Vec<&str> = at_head.lines().filter(|t| !t.is_empty()).collect();
    if requested.is_empty() {
        match tags.as_slice() {
            [one] => Ok((*one).to_owned()),
            [] => bail!("--tag: the private HEAD has no tag; tag the release commit first"),
            many => bail!(
                "--tag: the private HEAD has several tags ({}); name one",
                many.join(", ")
            ),
        }
    } else if tags.contains(&requested) {
        Ok(requested.to_owned())
    } else {
        bail!("--tag {requested}: that tag does not point at the private HEAD")
    }
}

fn run(root: &Path, apply: bool, push: bool, prune: bool, tag: Option<String>) -> Result<Report> {
    let cfg = load_config(root)?;
    let public = public_dir(root, &cfg)?;
    let head = git(root, &["rev-parse", "--short", "HEAD"])?;
    let dirty = git(root, &["status", "--porcelain"])?;
    if !dirty.is_empty() {
        eprintln!(
            "note: uncommitted changes are not exported ({} paths)",
            dirty.lines().count()
        );
    }

    let stage = tempdir()?;
    let result = (|| {
        extract_head(root, &stage)?;
        let (kept, dropped_markdown, excluded) = filter(&stage, &cfg)?;
        let mut leaks = scan(&stage, &kept, &cfg)?;
        if cfg.nym {
            leaks.extend(nym_scan(&stage, &kept, &cfg)?);
        }
        leaks.extend(dangling_links(&stage, &kept)?);
        let current = if public.join(".git").exists() {
            tree_hashes(&public)?
        } else {
            BTreeMap::new()
        };
        let staged = hashes_of(&stage, &kept)?;
        let added: Vec<String> = staged
            .keys()
            .filter(|p| !current.contains_key(*p))
            .cloned()
            .collect();
        let removed: Vec<String> = current
            .keys()
            .filter(|p| !staged.contains_key(*p))
            .cloned()
            .collect();
        let changed: Vec<String> = staged
            .iter()
            .filter(|(p, h)| current.get(*p).is_some_and(|c| c != *h))
            .map(|(p, _)| p.clone())
            .collect();
        let mut report = Report {
            repo: root.display().to_string(),
            head: head.clone(),
            public_dir: public.display().to_string(),
            files: kept.len(),
            dropped_markdown,
            excluded,
            added,
            removed,
            changed,
            leaks,
            extra_branches: if public.join(".git").exists() {
                remote_branches(&public)
            } else {
                Vec::new()
            },
            pruned_branches: Vec::new(),
            committed: None,
            tagged: None,
            pushed: false,
        };
        if apply && report.leaks.is_empty() {
            if let Some(name) = &tag {
                check_tag_free(&public, name, &report)?;
            }
            report.committed = write_public(root, &cfg, &public, &stage, &kept, &head)?;
            if let Some(name) = &tag {
                tag_snapshot(&public, name, &head)?;
                report.tagged = Some(name.clone());
            }
            if push {
                // Also pushes an earlier snapshot that was committed but not pushed.
                let range = format!("origin/{PUBLIC_BRANCH}..{PUBLIC_BRANCH}");
                let ahead = git(&public, &["rev-list", "--count", &range])
                    .unwrap_or_else(|_| "1".to_owned());
                if ahead != "0" {
                    git(&public, &["push", "origin", PUBLIC_BRANCH])?;
                    report.pushed = true;
                }
                // The tag goes after main, so the release builds a commit main already has.
                if let Some(name) = &report.tagged {
                    git(&public, &["push", "origin", &format!("refs/tags/{name}")])?;
                    report.pushed = true;
                }
                if prune {
                    for branch in remote_branches(&public) {
                        git(&public, &["push", "origin", "--delete", &branch])?;
                        report.pruned_branches.push(branch);
                    }
                }
            }
        }
        Ok(report)
    })();
    let _ = fs::remove_dir_all(&stage);
    result
}

fn extract_head(root: &Path, stage: &Path) -> Result<()> {
    let archive = Command::new("git")
        .current_dir(root)
        .args(["archive", "--format=tar", "HEAD"])
        .stdout(Stdio::piped())
        .spawn()
        .context("running git archive")?;
    let status = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(stage)
        .stdin(archive.stdout.context("git archive produced no output")?)
        .status()
        .context("running tar")?;
    if !status.success() {
        bail!("extracting HEAD failed");
    }
    Ok(())
}

fn matches(patterns: &[Pattern], rel: &str) -> bool {
    let options = MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    // A pattern naming a directory excludes everything beneath it.
    let mut candidate = Some(Path::new(rel));
    while let Some(path) = candidate {
        if patterns.iter().any(|p| p.matches_path_with(path, options)) {
            return true;
        }
        candidate = path.parent().filter(|p| !p.as_os_str().is_empty());
    }
    false
}

fn compile(globs: &[String]) -> Result<Vec<Pattern>> {
    globs
        .iter()
        .map(|g| {
            Pattern::new(g.trim_end_matches('/')).with_context(|| format!("invalid glob `{g}`"))
        })
        .collect()
}

/// Decide each staged file; delete the ones that stay private.
fn filter(stage: &Path, cfg: &PublishConfig) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
    let docs = compile(&cfg.docs)?;
    let mut exclude = compile(&cfg.exclude)?;
    exclude.push(Pattern::new(CONFIG_FILE)?);
    let (mut kept, mut dropped, mut excluded) = (Vec::new(), Vec::new(), Vec::new());
    for rel in files_under(stage)? {
        let path = Path::new(&rel);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let private_dir = path
            .components()
            .any(|c| ALWAYS_EXCLUDED_DIRS.contains(&c.as_os_str().to_str().unwrap_or_default()));
        let keep = if private_dir || AGENT_FILES.contains(&name) || matches(&exclude, &rel) {
            excluded.push(rel.clone());
            false
        } else if MARKDOWN.contains(&ext.as_str()) && !matches(&docs, &rel) {
            dropped.push(rel.clone());
            false
        } else {
            true
        };
        if keep {
            kept.push(rel);
        } else {
            fs::remove_file(stage.join(path))?;
        }
    }
    Ok((kept, dropped, excluded))
}

fn files_under(dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(dir).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }
        let rel = entry.path().strip_prefix(dir)?;
        if rel
            .components()
            .next()
            .is_some_and(|c| c.as_os_str() == ".git")
        {
            continue;
        }
        out.push(rel.to_string_lossy().replace('\\', "/"));
    }
    out.sort();
    Ok(out)
}

fn rules(cfg: &PublishConfig) -> Result<Vec<(String, Regex)>> {
    let mut rules = vec![
        (
            "home-path".to_owned(),
            Regex::new(r"(?:/Users/|/home/)([A-Za-z][A-Za-z0-9._-]*)")?,
        ),
        (
            "tailnet-address".to_owned(),
            Regex::new(r"\b100\.(?:6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.\d{1,3}\.\d{1,3}\b")?,
        ),
        (
            "private-key".to_owned(),
            Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")?,
        ),
    ];
    if let Some(user) = std::env::var_os("USER")
        .and_then(|u| u.into_string().ok())
        .filter(|u| u.len() > 2)
    {
        rules.push((
            "local-user".to_owned(),
            Regex::new(&format!(r"(?i)\b{}\b", regex::escape(&user)))?,
        ));
    }
    for term in cfg.forbidden.iter().filter(|t| !t.trim().is_empty()) {
        rules.push((
            format!("forbidden:{term}"),
            Regex::new(&format!("(?i){}", regex::escape(term.trim())))?,
        ));
    }
    Ok(rules)
}

fn scan(stage: &Path, kept: &[String], cfg: &PublishConfig) -> Result<Vec<Leak>> {
    let rules = rules(cfg)?;
    let allow: BTreeSet<&str> = cfg.allow.iter().map(String::as_str).collect();
    let mut leaks = Vec::new();
    for rel in kept {
        let bytes = match fs::read(stage.join(rel)) {
            Ok(b) => b,
            Err(_) => continue, // dangling symlink
        };
        if bytes.iter().take(8192).any(|b| *b == 0) {
            continue; // binary
        }
        let text = String::from_utf8_lossy(&bytes);
        for (number, line) in text.lines().enumerate() {
            for (rule, re) in &rules {
                for m in re.captures_iter(line) {
                    let whole = m.get(0).map_or("", |g| g.as_str());
                    let user = m.get(1).map(|g| g.as_str());
                    if allow.contains(whole) || user.is_some_and(|u| BENIGN_USERS.contains(&u)) {
                        continue;
                    }
                    leaks.push(Leak {
                        path: rel.clone(),
                        line: number + 1,
                        rule: rule.clone(),
                        text: whole.to_owned(),
                    });
                }
            }
        }
    }
    Ok(leaks)
}

/// Relative markdown links whose target is not part of the export (a private
/// decision record, an agent file) would be broken on the public side.
fn dangling_links(stage: &Path, kept: &[String]) -> Result<Vec<Leak>> {
    let link = Regex::new(r"\]\(([^)\s#]+)(?:#[^)]*)?\)")?;
    let present: BTreeSet<&str> = kept.iter().map(String::as_str).collect();
    let mut leaks = Vec::new();
    for rel in kept {
        let ext = Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !MARKDOWN.contains(&ext.as_str()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(stage.join(rel)) else {
            continue;
        };
        let base = Path::new(rel).parent().unwrap_or(Path::new(""));
        for (number, line) in text.lines().enumerate() {
            for m in link.captures_iter(line) {
                let target = &m[1];
                if target.contains("://")
                    || target.starts_with("mailto:")
                    || target.starts_with('/')
                {
                    continue;
                }
                let Some(resolved) = normalize(&base.join(target)) else {
                    continue;
                };
                let as_dir = format!("{}/", resolved.trim_end_matches('/'));
                let exists = present.contains(resolved.as_str())
                    || present.iter().any(|p| p.starts_with(&as_dir));
                if !exists {
                    leaks.push(Leak {
                        path: rel.clone(),
                        line: number + 1,
                        rule: "dangling-link".to_owned(),
                        text: target.to_owned(),
                    });
                }
            }
        }
    }
    Ok(leaks)
}

/// Resolve `.` and `..` lexically; `None` when the path leaves the root.
fn normalize(path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop()?;
            }
            std::path::Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

#[derive(Deserialize)]
struct NymFinding {
    pattern_name: String,
    matched_text: String,
    start: usize,
}

/// One `nym detect` call over all text files (it takes seconds to start),
/// findings mapped back to file and line by byte offset.
fn nym_scan(stage: &Path, kept: &[String], cfg: &PublishConfig) -> Result<Vec<Leak>> {
    let mut buffer = Vec::new();
    let mut spans: Vec<(usize, &String)> = Vec::new();
    for rel in kept {
        let Ok(bytes) = fs::read(stage.join(rel)) else {
            continue;
        };
        // nym reads UTF-8 only; other files are still covered by the built-in scan.
        if bytes.iter().take(8192).any(|b| *b == 0) || std::str::from_utf8(&bytes).is_err() {
            continue;
        }
        spans.push((buffer.len(), rel));
        buffer.extend_from_slice(&bytes);
        buffer.push(b'\n');
    }
    let input = stage.with_extension("nym-input");
    fs::write(&input, &buffer)?;
    let output = Command::new("nym")
        .arg("detect")
        .arg("--json")
        .args(&cfg.nym_args)
        .arg(&input)
        .output();
    let _ = fs::remove_file(&input);
    let output = output.context("`nym = true` in publish.toml but nym is not runnable")?;
    if !output.status.success() {
        bail!(
            "nym detect failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let findings: Vec<NymFinding> =
        serde_json::from_slice(&output.stdout).context("parsing nym output")?;
    let allow: BTreeSet<&str> = cfg.allow.iter().map(String::as_str).collect();
    let mut leaks = Vec::new();
    for f in findings {
        if allow.contains(f.matched_text.as_str()) {
            continue;
        }
        let index = spans
            .partition_point(|(offset, _)| *offset <= f.start)
            .saturating_sub(1);
        let Some((offset, rel)) = spans.get(index) else {
            continue;
        };
        let line = buffer[*offset..f.start.min(buffer.len())]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1;
        leaks.push(Leak {
            path: (*rel).clone(),
            line,
            rule: format!("nym:{}", f.pattern_name),
            text: f.matched_text,
        });
    }
    Ok(leaks)
}

/// Before anything is written: a tag that already exists publicly is only
/// acceptable when this export changes nothing and the tag is on public HEAD.
fn check_tag_free(public: &Path, name: &str, report: &Report) -> Result<()> {
    if !public.join(".git").exists() {
        return Ok(());
    }
    let local = git(
        public,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/tags/{name}^{{commit}}"),
        ],
    )
    .ok();
    let remote = git(
        public,
        &[
            "ls-remote",
            "--tags",
            "origin",
            &format!("refs/tags/{name}*"),
        ],
    )
    .unwrap_or_default();
    let remote = remote
        .lines()
        .find(|l| l.ends_with("^{}"))
        .or_else(|| remote.lines().next())
        .and_then(|l| l.split_whitespace().next())
        .map(str::to_owned);
    let Some(existing) = local.or(remote) else {
        return Ok(());
    };
    let unchanged =
        report.added.is_empty() && report.removed.is_empty() && report.changed.is_empty();
    let head = git(public, &["rev-parse", "HEAD"]).unwrap_or_default();
    if unchanged && existing == head {
        return Ok(());
    }
    bail!(
        "tag {name} already exists on another public commit ({existing}); a release tag is never moved"
    )
}

/// Tag the public HEAD; an existing tag is accepted only on the same commit.
fn tag_snapshot(public: &Path, name: &str, head: &str) -> Result<()> {
    let snapshot = git(public, &["rev-parse", "HEAD"])?;
    let existing = git(
        public,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/tags/{name}^{{commit}}"),
        ],
    )
    .ok();
    let remote = git(
        public,
        &[
            "ls-remote",
            "--tags",
            "origin",
            &format!("refs/tags/{name}^{{}}"),
        ],
    )
    .unwrap_or_default();
    let remote_commit = remote
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .or_else(|| {
            git(
                public,
                &[
                    "ls-remote",
                    "--tags",
                    "origin",
                    &format!("refs/tags/{name}"),
                ],
            )
            .ok()
            .and_then(|r| r.split_whitespace().next().map(str::to_owned))
        });
    for found in [existing, remote_commit].into_iter().flatten() {
        if found != snapshot {
            bail!(
                "tag {name} already exists on another public commit ({found}); a release tag is never moved"
            );
        }
    }
    if git(
        public,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/tags/{name}"),
        ],
    )
    .is_err()
    {
        git(
            public,
            &[
                "tag",
                "-a",
                name,
                "-m",
                &format!("Release {name}"),
                "-m",
                &format!("Exported from {head}."),
            ],
        )?;
    }
    Ok(())
}

fn hashes_of(dir: &Path, files: &[String]) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for rel in files {
        let path = dir.join(rel);
        let meta = fs::symlink_metadata(&path)?;
        let bytes = if meta.file_type().is_symlink() {
            fs::read_link(&path)?
                .to_string_lossy()
                .into_owned()
                .into_bytes()
        } else {
            fs::read(&path)?
        };
        out.insert(rel.clone(), format!("{:x}", Sha256::digest(&bytes)));
    }
    Ok(out)
}

fn tree_hashes(public: &Path) -> Result<BTreeMap<String, String>> {
    hashes_of(public, &files_under(public)?)
}

fn write_public(
    root: &Path,
    cfg: &PublishConfig,
    public: &Path,
    stage: &Path,
    kept: &[String],
    head: &str,
) -> Result<Option<String>> {
    if !public.join(".git").exists() {
        if let Some(parent) = public.parent() {
            fs::create_dir_all(parent)?;
        }
        match &cfg.public_remote {
            Some(remote) => {
                let status = Command::new("git")
                    .arg("clone")
                    .arg("-q")
                    .arg(remote)
                    .arg(public)
                    .status()?;
                if !status.success() {
                    bail!("cloning {remote} into {} failed", public.display());
                }
            }
            None => {
                fs::create_dir_all(public)?;
                git(public, &["init", "-q", "-b", "main"])?;
            }
        }
    }
    if !git(public, &["status", "--porcelain"])?.is_empty() {
        bail!(
            "{} has uncommitted changes; resolve them first",
            public.display()
        );
    }
    // A fresh `git init` has no commit yet; anything else must be on main.
    if git(public, &["rev-parse", "HEAD"]).is_ok() {
        let branch = git(public, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        if branch != PUBLIC_BRANCH {
            bail!(
                "{} is on `{branch}`; the public mirror publishes `{PUBLIC_BRANCH}` only",
                public.display()
            );
        }
    }
    let private_origin = git(root, &["remote", "get-url", "origin"]).unwrap_or_default();
    let public_remotes = git(public, &["remote", "-v"]).unwrap_or_default();
    if !private_origin.is_empty() && public_remotes.contains(&private_origin) {
        bail!(
            "{} points at the private origin {private_origin}",
            public.display()
        );
    }

    for rel in files_under(public)? {
        if !kept.contains(&rel) {
            fs::remove_file(public.join(&rel))?;
        }
    }
    remove_empty_dirs(public)?;
    for rel in kept {
        let from = stage.join(rel);
        let to = public.join(rel);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::symlink_metadata(&to).is_ok() {
            fs::remove_file(&to)?;
        }
        let meta = fs::symlink_metadata(&from)?;
        if meta.file_type().is_symlink() {
            copy_symlink(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    git(public, &["add", "-A"])?;
    if git(public, &["status", "--porcelain"])?.is_empty() {
        return Ok(None);
    }
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repository");
    let date = chrono::Local::now().format("%Y-%m-%d");
    git(
        public,
        &[
            "commit",
            "-q",
            "-m",
            &format!("{name} snapshot {date}"),
            "-m",
            &format!("Exported from {head}."),
        ],
    )?;
    Ok(Some(git(public, &["rev-parse", "--short", "HEAD"])?))
}

#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
    Ok(())
}

#[cfg(not(unix))]
fn copy_symlink(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to)?;
    Ok(())
}

fn remove_empty_dirs(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() && !path.is_symlink() && path.file_name().is_some_and(|n| n != ".git") {
            remove_empty_dirs(&path)?;
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
        }
    }
    Ok(())
}

fn init(root: &Path) -> Result<()> {
    let path = root.join(CONFIG_FILE);
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    let tracked = git(root, &["ls-files"])?;
    let markdown: Vec<&str> = tracked
        .lines()
        .filter(|f| {
            let p = Path::new(f);
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            MARKDOWN.contains(&ext.as_str()) && !AGENT_FILES.contains(&name)
        })
        .collect();
    let mut out = String::from(
        "# byt publish: what of this private repository may appear in its public mirror.\n\
         # This file itself, agent/context files and .trx/.pi/.claude are never exported.\n\n\
         # public_dir = \"~/byteowlz-public/<name>\"\n\
         # public_remote = \"git@github.com:byteowlz/<name>.git\"\n\n\
         # Markdown allowed in the export; every other markdown file stays private.\n\
         docs = [\n",
    );
    for file in &markdown {
        let default_public = matches!(*file, "README.md" | "CHANGELOG.md");
        let prefix = if default_public { "  " } else { "  # " };
        out.push_str(&format!("{prefix}\"{file}\",\n"));
    }
    out.push_str("]\n\n# Further tracked paths left out (globs).\nexclude = []\n\n");
    out.push_str("# Case-insensitive terms that block the export.\nforbidden = []\n\n");
    out.push_str("# Exact leak-scan matches known to be harmless.\nallow = []\n\n");
    out.push_str(
        "# Also run `nym detect` over the export (slower; needs nym installed).\nnym = false\n",
    );
    let mut file = fs::File::create_new(&path)?;
    file.write_all(out.as_bytes())?;
    println!(
        "wrote {} ({} markdown files listed, README/CHANGELOG enabled)",
        path.display(),
        markdown.len()
    );
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .context("running git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn tempdir() -> Result<PathBuf> {
    // The clock alone is too coarse on macOS to keep parallel callers apart.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("byt-publish-{}-{n}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "git {args:?}");
    }

    fn private_repo(root: &Path, config: &str) {
        fs::create_dir_all(root.join("docs/adr")).unwrap();
        fs::create_dir_all(root.join(".trx")).unwrap();
        for (path, body) in [
            ("README.md", "readme, see [usage](docs/usage.md)"),
            ("CHANGELOG.md", "changes"),
            ("AGENTS.md", "agent rules"),
            ("CONTEXT.md", "glossary"),
            ("docs/usage.md", "usage"),
            ("docs/adr/0001-x.md", "decision"),
            (".trx/issues.jsonl", "{}"),
            ("src.rs", "fn main() {}"),
            ("publish.toml", config),
        ] {
            fs::write(root.join(path), body).unwrap();
        }
        sh(root, &["init", "-q", "-b", "main"]);
        sh(root, &["add", "-A"]);
        sh(root, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn export_keeps_only_allowed_markdown_and_never_agent_files() {
        let base = tempdir().unwrap();
        let root = base.join("work");
        let public = base.join("public");
        fs::create_dir_all(&root).unwrap();
        let config = format!(
            "public_dir = \"{}\"\ndocs = [\"README.md\", \"CHANGELOG.md\", \"docs/usage.md\"]\n",
            public.display()
        );
        private_repo(&root, &config);

        let report = run(&root, true, false, false, None).unwrap();
        assert!(report.leaks.is_empty(), "{:?}", report.leaks);
        assert!(report.committed.is_some());
        let mut files = files_under(&public).unwrap();
        files.sort();
        assert_eq!(
            files,
            ["CHANGELOG.md", "README.md", "docs/usage.md", "src.rs"]
        );
        assert!(
            report
                .dropped_markdown
                .contains(&"docs/adr/0001-x.md".to_owned())
        );
        assert!(report.excluded.contains(&"AGENTS.md".to_owned()));

        // A second export with no changes commits nothing.
        let again = run(&root, true, false, false, None).unwrap();
        assert!(again.committed.is_none());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn leaks_block_the_export_and_allow_entries_clear_them() {
        let base = tempdir().unwrap();
        let root = base.join("work");
        let public = base.join("public");
        fs::create_dir_all(&root).unwrap();
        let config = format!(
            "public_dir = \"{}\"\ndocs = [\"README.md\"]\nforbidden = [\"Acme Internal\"]\n",
            public.display()
        );
        private_repo(&root, &config);
        fs::write(
            root.join("leak.rs"),
            // Built at runtime so this source file does not trip its own scan.
            format!(
                "// see /Users/{}/x, host 100.{}.0.99, acme internal; /home/user is fine",
                "jdoe", 64
            ),
        )
        .unwrap();
        sh(&root, &["add", "-A"]);
        sh(&root, &["commit", "-q", "-m", "leak"]);

        let report = run(&root, true, false, false, None).unwrap();
        let rules: BTreeSet<&str> = report.leaks.iter().map(|l| l.rule.as_str()).collect();
        assert!(
            rules.contains("home-path")
                && rules.contains("tailnet-address")
                && rules.contains("forbidden:Acme Internal"),
            "{rules:?}"
        );
        assert!(report.leaks.iter().all(|l| l.text != "/home/user"));
        assert!(
            report.committed.is_none() && !public.exists(),
            "nothing written while leaks remain"
        );
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn nym_findings_map_to_file_and_line() {
        if which::which("nym").is_err() {
            eprintln!("nym not installed; skipped");
            return;
        }
        let base = tempdir().unwrap();
        let root = base.join("work");
        fs::create_dir_all(&root).unwrap();
        let config = format!(
            "public_dir = \"{}\"\ndocs = [\"README.md\"]\nnym = true\n",
            base.join("public").display()
        );
        private_repo(&root, &config);
        fs::write(root.join("b.rs"), "// ok\n// mail jane.doe@acme-corp.de\n").unwrap();
        sh(&root, &["add", "-A"]);
        sh(&root, &["commit", "-q", "-m", "mail"]);

        let report = run(&root, false, false, false, None).unwrap();
        let hit = report
            .leaks
            .iter()
            .find(|l| l.rule == "nym:email")
            .expect("nym finds the email");
        assert_eq!((hit.path.as_str(), hit.line), ("b.rs", 2));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn links_to_private_files_block_the_export() {
        let base = tempdir().unwrap();
        let root = base.join("work");
        fs::create_dir_all(&root).unwrap();
        let config = format!(
            "public_dir = \"{}\"\ndocs = [\"README.md\", \"docs/usage.md\"]\n",
            base.join("public").display()
        );
        private_repo(&root, &config);
        fs::write(
            root.join("docs/usage.md"),
            "[decision](adr/0001-x.md) [ok](../README.md) [web](https://x.example/a.md)",
        )
        .unwrap();
        sh(&root, &["add", "-A"]);
        sh(&root, &["commit", "-q", "-m", "links"]);

        let report = run(&root, false, false, false, None).unwrap();
        let dangling: Vec<&str> = report
            .leaks
            .iter()
            .filter(|l| l.rule == "dangling-link")
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(dangling, ["adr/0001-x.md"]);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn push_publishes_main_only_and_prunes_other_branches() {
        let base = tempdir().unwrap();
        let root = base.join("work");
        let remote = base.join("remote.git");
        let public = base.join("public");
        fs::create_dir_all(&root).unwrap();
        sh(
            &base,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
        );
        // The public remote already has main and a stray branch.
        let seed = base.join("seed");
        fs::create_dir_all(&seed).unwrap();
        sh(&seed, &["init", "-q", "-b", "main"]);
        fs::write(seed.join("old.txt"), "old").unwrap();
        sh(&seed, &["add", "-A"]);
        sh(&seed, &["commit", "-q", "-m", "old"]);
        sh(
            &seed,
            &["push", "-q", remote.to_str().unwrap(), "main", "main:stray"],
        );

        let config = format!(
            "public_dir = \"{}\"\npublic_remote = \"{}\"\ndocs = [\"README.md\", \"docs/usage.md\"]\n",
            public.display(),
            remote.display()
        );
        private_repo(&root, &config);
        let report = run(&root, true, true, true, None).unwrap();
        assert!(report.pushed, "{report:?}");
        assert_eq!(report.pruned_branches, ["stray"]);
        let heads = git(&base, &["ls-remote", "--heads", remote.to_str().unwrap()]).unwrap();
        assert_eq!(heads.lines().count(), 1, "{heads}");
        assert!(heads.ends_with("refs/heads/main"));
        assert!(
            !files_under(&public)
                .unwrap()
                .contains(&"old.txt".to_owned())
        );

        // A public checkout moved off main is refused.
        sh(&public, &["checkout", "-q", "-b", "side"]);
        fs::write(root.join("src.rs"), "fn main() { let _ = 1; }").unwrap();
        sh(&root, &["commit", "-qam", "change"]);
        let error = run(&root, true, false, false, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("publishes `main` only"), "{error}");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn release_tag_lands_on_the_snapshot_and_is_never_moved() {
        let base = tempdir().unwrap();
        let root = base.join("work");
        let remote = base.join("remote.git");
        let public = base.join("public");
        fs::create_dir_all(&root).unwrap();
        sh(
            &base,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
        );
        let config = format!(
            "public_dir = \"{}\"\npublic_remote = \"{}\"\ndocs = [\"README.md\", \"docs/usage.md\"]\n",
            public.display(),
            remote.display()
        );
        private_repo(&root, &config);
        assert!(release_tag(&root, "").is_err(), "untagged HEAD is refused");
        sh(&root, &["tag", "-a", "v1.0.0", "-m", "v1.0.0"]);
        assert!(
            release_tag(&root, "v9.9.9").is_err(),
            "a tag not at HEAD is refused"
        );
        let tag = release_tag(&root, "").unwrap();

        let report = run(&root, true, true, false, Some(tag)).unwrap();
        assert_eq!(report.tagged.as_deref(), Some("v1.0.0"));
        let main = git(
            &base,
            &["--git-dir", remote.to_str().unwrap(), "rev-parse", "main"],
        )
        .unwrap();
        let tagged = git(
            &base,
            &[
                "--git-dir",
                remote.to_str().unwrap(),
                "rev-parse",
                "v1.0.0^{commit}",
            ],
        )
        .unwrap();
        assert_eq!(main, tagged, "the public tag sits on the pushed snapshot");

        // Moving the private tag to a new commit must not move the public one.
        fs::write(root.join("src.rs"), "fn main() { let _ = 2; }").unwrap();
        sh(&root, &["commit", "-qam", "change"]);
        sh(&root, &["tag", "-f", "-a", "v1.0.0", "-m", "v1.0.0"]);
        let error = run(&root, true, true, false, Some("v1.0.0".into()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("never moved"), "{error}");
        let unpushed = git(&public, &["rev-list", "--count", "origin/main..main"]).unwrap();
        assert_eq!(unpushed, "0", "nothing is written when the tag check fails");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn agent_files_cannot_be_allowlisted() {
        let base = tempdir().unwrap();
        fs::write(base.join(CONFIG_FILE), "docs = [\"AGENTS.md\"]\n").unwrap();
        let error = load_config(&base).unwrap_err().to_string();
        assert!(error.contains("never published"), "{error}");
        fs::remove_dir_all(&base).unwrap();
    }
}
