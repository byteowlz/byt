//! `byt release` — produce spec-conformant release artifacts for byteowlz apps.
//!
//! byt owns the parts that must be uniform across every app — staging layout,
//! archive naming, `checksums.txt`, and publishing — and delegates only the
//! genuinely language-specific step (compilation) to a per-language backend. So
//! every byteowlz app ships identically-shaped artifacts that
//! `oqto-setup acquire` verifies. See ADR-0021 (the spec) and ADR-0022 (byt is
//! distributed as a GHCR container so a runner has byt + toolchain) in the oqto
//! repo.
//!
//! Artifact contract (ADR-0021):
//! - name: `{name}-v{version}-{target-triple}.tar.gz`
//! - layout: one top-level `{name}-v{version}-{triple}/` dir with `bin/<exes>` + extra files
//! - checksums: one `checksums.txt` per release (`<sha256>  <filename>` lines)

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::RuntimeContext;

/// Staging/output directory, relative to the repo root.
const DIST_DIR: &str = "dist/release";
/// Per-repo release config consumed by `byt release`.
const CONFIG_FILE: &str = "byt.release.toml";
const DEFAULT_EXTRA: &[&str] = &["LICENSE", "README.md"];
const DEFAULT_TARGETS: &[&str] = &["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"];
/// glibc floor for linux-gnu release artifacts (ADR-0021): built via
/// `cargo-zigbuild --target <triple>.<floor>` so binaries run on older systems
/// (RHEL8 / Ubuntu 18.04+). Dynamic-link only — host's patched glibc at runtime.
const GLIBC_FLOOR: &str = "2.28";

#[derive(Debug, clap::Subcommand)]
pub enum ReleaseCommand {
    /// Compile + stage + archive one target (or all configured targets).
    Build {
        /// Build a single target triple instead of all configured ones.
        #[arg(long)]
        target: Option<String>,
        /// Only build configured targets for this OS (`linux`/`macos`/`windows`).
        /// Lets the hybrid CI split build linux in-container and macOS natively.
        #[arg(long)]
        os: Option<String>,
    },
    /// Print configured target triples (optionally filtered by `--os`), one per
    /// line. Used by CI to decide whether a per-OS build job is needed.
    Targets {
        /// Filter to targets for this OS (`linux`/`macos`/`windows`).
        #[arg(long)]
        os: Option<String>,
    },
    /// Generate checksums.txt over the dist directory.
    Checksums,
    /// Create the GitHub release for the tag and upload all artifacts.
    Publish {
        /// Release tag (default: $GITHUB_REF_NAME or v{version}).
        #[arg(long)]
        tag: Option<String>,
    },
    /// Verify staged artifacts in a directory against its checksums.txt.
    Verify {
        /// Directory containing *.tar.gz + checksums.txt.
        path: PathBuf,
    },
    /// Full pipeline: build all targets + checksums + publish + packages.
    Run {
        /// Release tag (default: $GITHUB_REF_NAME or v{version}).
        #[arg(long)]
        tag: Option<String>,
    },
    /// Publish downstream packages (Homebrew dispatch + AUR) for a release that
    /// already exists; reads shas from the local `dist/release/checksums.txt`.
    Packages {
        /// Release tag (default: $GITHUB_REF_NAME or v{version}).
        #[arg(long)]
        tag: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Lang {
    Rust,
    Go,
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    release: ReleaseSection,
}

#[derive(Debug, Deserialize)]
struct ReleaseSection {
    /// Artifact/app name (the `{name}` in the artifact filename).
    name: String,
    /// Compile backend.
    lang: Lang,
    /// Binaries to ship; defaults to `[name]` when empty.
    #[serde(default)]
    bins: Vec<String>,
    /// Version override; resolved from tag/Cargo.toml when absent.
    #[serde(default)]
    version: Option<String>,
    /// Target triples; defaults to linux x86_64 + aarch64.
    #[serde(default)]
    targets: Vec<String>,
    /// Extra files copied to the staging root; defaults to LICENSE + README.md.
    #[serde(default)]
    extra: Vec<String>,
    /// Sign checksums.txt (not yet implemented).
    #[serde(default)]
    sign: bool,
    /// Homebrew publishing (opt-in). Fires a `repository_dispatch` at the tap.
    #[serde(default)]
    homebrew: Option<Homebrew>,
    /// AUR publishing (opt-in). Generates PKGBUILD + .SRCINFO and pushes to AUR.
    #[serde(default)]
    aur: Option<Aur>,
}

/// `[release.homebrew]` — publish by dispatching to the tap repo, which owns
/// formula generation. byt only fires the event (with the App token).
#[derive(Debug, Deserialize)]
struct Homebrew {
    /// Tap repo (`owner/name`).
    #[serde(default = "default_tap")]
    tap: String,
    /// Formula name; defaults to the release `name`.
    #[serde(default)]
    formula: Option<String>,
    /// `repository_dispatch` event type the tap's workflow listens for.
    #[serde(default = "default_event_type")]
    event_type: String,
}

/// `[release.aur]` — publish the `-bin` package to the AUR. byt generates a
/// PKGBUILD + .SRCINFO for the current release artifacts and pushes over SSH;
/// no Arch tooling (`makepkg`) is required since every field is known here.
#[derive(Debug, Deserialize)]
struct Aur {
    /// AUR package name; defaults to `{name}-bin`.
    #[serde(default)]
    pkgname: Option<String>,
    /// `pkgdesc` — required (there is no sensible default).
    pkgdesc: String,
    /// SPDX-ish license id for the `license=()` field.
    #[serde(default = "default_license")]
    license: String,
    /// Upstream URL; defaults to `https://github.com/{GITHUB_REPOSITORY}`.
    #[serde(default)]
    url: Option<String>,
    /// `provides=()`; defaults to `[name]`.
    #[serde(default)]
    provides: Vec<String>,
    /// `conflicts=()`; defaults to `[name]`.
    #[serde(default)]
    conflicts: Vec<String>,
    /// `# Maintainer:` line; defaults to a generic byteowlz maintainer.
    #[serde(default = "default_maintainer")]
    maintainer: String,
}

fn default_tap() -> String {
    "byteowlz/homebrew-tap".to_string()
}
fn default_event_type() -> String {
    "update-formula".to_string()
}
fn default_license() -> String {
    "MIT".to_string()
}
fn default_maintainer() -> String {
    "byteowlz <dev@byteowlz.com>".to_string()
}

/// (CARCH, target-triple) pairs byt publishes to the AUR. A source is emitted
/// only when its artifact exists in `checksums.txt`.
const AUR_ARCHES: &[(&str, &str)] = &[
    ("x86_64", "x86_64-unknown-linux-gnu"),
    ("aarch64", "aarch64-unknown-linux-gnu"),
];

impl ReleaseSection {
    fn targets(&self) -> Vec<String> {
        if self.targets.is_empty() {
            DEFAULT_TARGETS.iter().map(|s| s.to_string()).collect()
        } else {
            self.targets.clone()
        }
    }
    fn extra(&self) -> Vec<String> {
        if self.extra.is_empty() {
            DEFAULT_EXTRA.iter().map(|s| s.to_string()).collect()
        } else {
            self.extra.clone()
        }
    }
    fn bins(&self) -> Vec<String> {
        if self.bins.is_empty() {
            vec![self.name.clone()]
        } else {
            self.bins.clone()
        }
    }
}

// ---- pure helpers (unit-tested) -------------------------------------------

fn archive_filename(name: &str, version: &str, triple: &str) -> String {
    format!("{name}-v{version}-{triple}.tar.gz")
}

fn staging_dirname(name: &str, version: &str, triple: &str) -> String {
    format!("{name}-v{version}-{triple}")
}

/// Coarse OS bucket for a target triple (for the hybrid CI's per-OS build jobs).
fn os_of(triple: &str) -> &'static str {
    if triple.contains("linux") {
        "linux"
    } else if triple.contains("apple") || triple.contains("darwin") {
        "macos"
    } else if triple.contains("windows") {
        "windows"
    } else {
        "other"
    }
}

/// Resolve which triples to build: an explicit `--target` wins; otherwise the
/// configured targets, optionally filtered to one `--os`.
fn select_targets(cfg: &ReleaseSection, target: Option<String>, os: Option<&str>) -> Vec<String> {
    if let Some(t) = target {
        return vec![t];
    }
    let all = cfg.targets();
    match os {
        Some(os) => all.into_iter().filter(|t| os_of(t) == os).collect(),
        None => all,
    }
}

/// Map a Rust target triple to the Go `GOOS`/`GOARCH` the Go backend needs.
fn go_target(triple: &str) -> Result<(&'static str, &'static str)> {
    let os = if triple.contains("linux") {
        "linux"
    } else if triple.contains("darwin") || triple.contains("apple") {
        "darwin"
    } else if triple.contains("windows") {
        "windows"
    } else {
        bail!("unsupported OS in target triple: {triple}");
    };
    let arch = if triple.starts_with("x86_64") {
        "amd64"
    } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
        "arm64"
    } else {
        bail!("unsupported arch in target triple: {triple}");
    };
    Ok((os, arch))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

/// Render a `checksums.txt` body (`<sha>  <filename>` lines, sorted by name).
fn checksums_body(entries: &[(String, String)]) -> String {
    let mut entries = entries.to_vec();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::new();
    for (filename, sha) in entries {
        use std::fmt::Write;
        let _ = writeln!(out, "{sha}  {filename}");
    }
    out
}

fn strip_v(s: &str) -> String {
    s.trim_start_matches('v').to_string()
}

/// Extract the first `version = "x"` under `[package]` from a Cargo.toml body.
fn parse_cargo_version(text: &str) -> Option<String> {
    let mut in_pkg = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_pkg = line == "[package]";
            continue;
        }
        if in_pkg
            && let Some(rest) = line.strip_prefix("version")
            && let Some(v) = rest.split('"').nth(1)
        {
            return Some(v.to_string());
        }
    }
    None
}

// ---- config / version -----------------------------------------------------

fn load_config(root: &Path) -> Result<ReleaseSection> {
    let path = root.join(CONFIG_FILE);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("reading {} (run from the repo root)", path.display()))?;
    let cfg: ConfigFile =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg.release)
}

/// Resolve the release version: explicit `--tag` → `$RELEASE_TAG` →
/// `$GITHUB_REF_NAME` → `[release].version` → `Cargo.toml`. All forms are
/// normalized without a leading `v`.
///
/// `RELEASE_TAG` is checked before `GITHUB_REF_NAME` because the latter is a
/// reserved variable that GitHub Actions refuses to let a workflow override: on
/// a `workflow_dispatch` run it is the *branch* (e.g. `master`), not the release
/// tag, so relying on it silently mis-versions dispatched releases. CI sets the
/// non-reserved `RELEASE_TAG` to the intended tag instead.
fn resolve_version(cfg: &ReleaseSection, tag: Option<&str>, root: &Path) -> Result<String> {
    if let Some(tag) = tag {
        return Ok(strip_v(tag));
    }
    for key in ["RELEASE_TAG", "GITHUB_REF_NAME"] {
        if let Ok(reff) = std::env::var(key)
            && !reff.is_empty()
        {
            return Ok(strip_v(&reff));
        }
    }
    if let Some(version) = &cfg.version {
        return Ok(strip_v(version));
    }
    let cargo = root.join("Cargo.toml");
    if cargo.exists() {
        let text = fs::read_to_string(&cargo)?;
        if let Some(version) = parse_cargo_version(&text) {
            return Ok(version);
        }
    }
    bail!("could not resolve version: pass --tag, set [release].version, or $RELEASE_TAG")
}

// ---- command execution ----------------------------------------------------

fn render(cmd: &Command) -> String {
    let prog = cmd.get_program().to_string_lossy();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    format!("{prog} {}", args.join(" "))
}

/// Run a command, or print it under `--dry-run`.
fn run(ctx: &RuntimeContext, cmd: &mut Command) -> Result<()> {
    if ctx.common.dry_run {
        println!("  [dry-run] {}", render(cmd));
        return Ok(());
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawning `{}`", render(cmd)))?;
    if !status.success() {
        bail!("command failed: `{}`", render(cmd));
    }
    Ok(())
}

fn copy_exec(ctx: &RuntimeContext, src: &Path, dest: &Path) -> Result<()> {
    if ctx.common.dry_run {
        println!(
            "  [dry-run] install {} -> {}",
            src.display(),
            dest.display()
        );
        return Ok(());
    }
    fs::copy(src, dest)
        .with_context(|| format!("copying {} -> {}", src.display(), dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(dest)?.permissions();
        perm.set_mode(0o755);
        fs::set_permissions(dest, perm)?;
    }
    Ok(())
}

fn copy_file(ctx: &RuntimeContext, src: &Path, dest: &Path) -> Result<()> {
    if ctx.common.dry_run {
        println!("  [dry-run] copy {} -> {}", src.display(), dest.display());
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, dest)
        .with_context(|| format!("copying {} -> {}", src.display(), dest.display()))?;
    Ok(())
}

// ---- compile backends (the only language-specific step) -------------------

/// The target passed to `cargo zigbuild`: linux-gnu targets get a glibc floor
/// (`<triple>.<GLIBC_FLOOR>`) so the artifact runs on older systems; other
/// targets are unchanged. The rust output dir + artifact name keep the plain
/// triple (cargo-zigbuild strips the glibc suffix for cargo).
fn zig_build_target(triple: &str) -> String {
    if triple.ends_with("-linux-gnu") {
        format!("{triple}.{GLIBC_FLOOR}")
    } else {
        triple.to_string()
    }
}

fn compile_rust(ctx: &RuntimeContext, root: &Path, triple: &str) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root);
    if triple.ends_with("-linux-gnu") {
        // linux-gnu is built in the Linux container and needs the glibc floor
        // (ADR-0021), so it goes through cargo-zigbuild.
        cmd.arg("zigbuild")
            .arg("--release")
            .arg("--target")
            .arg(zig_build_target(triple));
    } else {
        // Everything else (notably *-apple-darwin) is built natively on its own
        // runner: plain cargo avoids zig's macOS-framework linking pitfalls and
        // there is no glibc floor to apply.
        cmd.arg("build")
            .arg("--release")
            .arg("--target")
            .arg(triple);
    }
    run(ctx, &mut cmd)
}

fn compile_go(
    ctx: &RuntimeContext,
    cfg: &ReleaseSection,
    root: &Path,
    bin_dir: &Path,
    triple: &str,
) -> Result<()> {
    let (goos, goarch) = go_target(triple)?;
    for bin in cfg.bins() {
        let mut cmd = Command::new("go");
        cmd.current_dir(root)
            .env("GOOS", goos)
            .env("GOARCH", goarch)
            .env("CGO_ENABLED", "0")
            .arg("build")
            .arg("-o")
            .arg(bin_dir.join(&bin))
            // convention: each shipped binary has a `./cmd/<bin>` main package.
            .arg(format!("./cmd/{bin}"));
        run(ctx, &mut cmd)?;
    }
    Ok(())
}

// ---- pipeline -------------------------------------------------------------

fn build_target(
    ctx: &RuntimeContext,
    cfg: &ReleaseSection,
    root: &Path,
    version: &str,
    triple: &str,
    dist: &Path,
) -> Result<PathBuf> {
    let staging = dist.join(staging_dirname(&cfg.name, version, triple));
    let bin_dir = staging.join("bin");
    if !ctx.common.dry_run {
        fs::create_dir_all(&bin_dir).with_context(|| format!("creating {}", bin_dir.display()))?;
    }

    // 1. compile (delegated to the language backend)
    match cfg.lang {
        Lang::Rust => compile_rust(ctx, root, triple)?,
        Lang::Go => compile_go(ctx, cfg, root, &bin_dir, triple)?,
    }

    // 2. stage binaries into bin/ (Go already built straight into bin_dir)
    if cfg.lang == Lang::Rust {
        for bin in cfg.bins() {
            let built = root.join("target").join(triple).join("release").join(&bin);
            copy_exec(ctx, &built, &bin_dir.join(&bin))?;
        }
    }

    // 3. extra files (LICENSE/README) at the staging root
    for extra in cfg.extra() {
        let src = root.join(&extra);
        if src.exists() {
            copy_file(ctx, &src, &staging.join(&extra))?;
        }
    }

    // 4. archive: a single top-level {name}-v{ver}-{triple}/ dir
    let archive = dist.join(archive_filename(&cfg.name, version, triple));
    let mut tar = Command::new("tar");
    tar.arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(dist)
        .arg(staging_dirname(&cfg.name, version, triple));
    run(ctx, &mut tar)?;
    println!("  built {}", archive.display());
    Ok(archive)
}

fn generate_checksums(ctx: &RuntimeContext, dist: &Path) -> Result<()> {
    if !dist.exists() {
        if ctx.common.dry_run {
            println!("  [dry-run] would write {}/checksums.txt", dist.display());
            return Ok(());
        }
        bail!(
            "dist dir {} does not exist — run `byt release build` first",
            dist.display()
        );
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(dist).with_context(|| format!("reading {}", dist.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("gz") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            entries.push((name, sha256_file(&path)?));
        }
    }
    if entries.is_empty() {
        bail!("no .tar.gz artifacts in {} to checksum", dist.display());
    }
    let body = checksums_body(&entries);
    let out = dist.join("checksums.txt");
    if ctx.common.dry_run {
        println!("  [dry-run] write {}:\n{body}", out.display());
        return Ok(());
    }
    fs::write(&out, body).with_context(|| format!("writing {}", out.display()))?;
    println!("  wrote {} ({} artifacts)", out.display(), entries.len());
    Ok(())
}

fn publish(
    ctx: &RuntimeContext,
    cfg: &ReleaseSection,
    version: &str,
    tag: Option<&str>,
    dist: &Path,
) -> Result<()> {
    if cfg.sign {
        eprintln!("  note: [release].sign is set but signing is not yet implemented (ADR-0021)");
    }
    let tag = tag
        .map(str::to_string)
        .unwrap_or_else(|| format!("v{version}"));
    let checksums = dist.join("checksums.txt");
    if !checksums.exists() && !ctx.common.dry_run {
        bail!(
            "missing {} — run `byt release checksums` first",
            checksums.display()
        );
    }

    let mut cmd = Command::new("gh");
    cmd.arg("release")
        .arg("create")
        .arg(&tag)
        .arg("--title")
        .arg(&tag)
        .arg("--generate-notes");
    // In CI, target the repo explicitly so `gh` does not have to detect it from
    // the working-directory git checkout — which fails under container-job UID
    // mismatches ("dubious ownership"). Locally, GITHUB_REPOSITORY is unset and
    // gh falls back to the cwd repo as before.
    if let Ok(repo) = std::env::var("GITHUB_REPOSITORY")
        && !repo.is_empty()
    {
        cmd.arg("--repo").arg(repo);
    }
    if dist.exists() {
        for entry in fs::read_dir(dist)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("gz") {
                cmd.arg(&path);
            }
        }
    }
    cmd.arg(&checksums);
    run(ctx, &mut cmd)?;
    println!("  published {} {tag}", cfg.name);
    Ok(())
}

fn verify(dir: &Path) -> Result<()> {
    let checksums = dir.join("checksums.txt");
    let text = fs::read_to_string(&checksums)
        .with_context(|| format!("reading {}", checksums.display()))?;
    let mut count = 0usize;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(expected), Some(filename)) = (parts.next(), parts.next()) else {
            continue;
        };
        let filename = filename.trim_start_matches('*');
        let actual =
            sha256_file(&dir.join(filename)).with_context(|| format!("hashing {filename}"))?;
        if actual.eq_ignore_ascii_case(expected) {
            count += 1;
        } else {
            bail!("checksum mismatch for {filename}: expected {expected}, got {actual}");
        }
    }
    println!("  verified {count} artifact(s) in {}", dir.display());
    Ok(())
}

// ---- downstream packages (Homebrew + AUR) ---------------------------------

/// One AUR arch source: CARCH + the triple whose artifact backs it + its sha256.
struct AurArch {
    carch: &'static str,
    triple: &'static str,
    sha: String,
}

/// The `owner/name` this release belongs to (Homebrew payload + AUR URLs).
fn resolve_repo() -> Result<String> {
    std::env::var("GITHUB_REPOSITORY")
        .ok()
        .filter(|s| !s.is_empty())
        .context("GITHUB_REPOSITORY not set (needed for Homebrew/AUR publishing)")
}

/// Parse `dist/release/checksums.txt` into `(filename, sha256)` pairs.
fn read_checksums(dist: &Path) -> Result<Vec<(String, String)>> {
    let path = dist.join("checksums.txt");
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "reading {} (run `byt release checksums` first)",
            path.display()
        )
    })?;
    let mut out = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if let (Some(sha), Some(name)) = (it.next(), it.next()) {
            out.push((name.trim_start_matches('*').to_string(), sha.to_string()));
        }
    }
    Ok(out)
}

/// Run a command feeding `input` on stdin (used for `gh api --input -`).
fn run_stdin(ctx: &RuntimeContext, cmd: &mut Command, input: &str) -> Result<()> {
    if ctx.common.dry_run {
        println!("  [dry-run] {} <<< {input}", render(cmd));
        return Ok(());
    }
    cmd.stdin(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning `{}`", render(cmd)))?;
    {
        let mut stdin = child.stdin.take().context("capturing stdin")?;
        stdin.write_all(input.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("command failed: `{}`", render(cmd));
    }
    Ok(())
}

/// Homebrew: fire a `repository_dispatch` at the tap; the tap owns formula
/// generation. Authorized by the GitHub App token in `$HOMEBREW_TAP_TOKEN`
/// (the default `GITHUB_TOKEN` cannot write to the tap repo).
fn publish_homebrew(
    ctx: &RuntimeContext,
    hb: &Homebrew,
    name: &str,
    version: &str,
    repo: &str,
) -> Result<()> {
    let formula = hb.formula.clone().unwrap_or_else(|| name.to_string());
    let payload = serde_json::json!({
        "event_type": hb.event_type,
        "client_payload": { "formula": formula, "version": format!("v{version}"), "repo": repo },
    });
    let body = serde_json::to_string(&payload)?;

    let mut cmd = Command::new("gh");
    cmd.arg("api")
        .arg("--method")
        .arg("POST")
        .arg(format!("/repos/{}/dispatches", hb.tap))
        .arg("--input")
        .arg("-");
    match std::env::var("HOMEBREW_TAP_TOKEN") {
        Ok(tok) if !tok.is_empty() => {
            cmd.env("GH_TOKEN", tok);
        }
        _ if ctx.common.dry_run => {}
        _ => bail!(
            "HOMEBREW_TAP_TOKEN not set — the tap dispatch needs the release App token \
             (the default GITHUB_TOKEN cannot write to {})",
            hb.tap
        ),
    }
    run_stdin(ctx, &mut cmd, &body)?;
    println!("  dispatched {formula} formula update -> {}", hb.tap);
    Ok(())
}

fn quoted_list(items: &[String]) -> String {
    items
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Effective `provides`/`conflicts` (default to `[name]` when unset).
fn aur_provides(name: &str, aur: &Aur) -> Vec<String> {
    if aur.provides.is_empty() {
        vec![name.to_string()]
    } else {
        aur.provides.clone()
    }
}
fn aur_conflicts(name: &str, aur: &Aur) -> Vec<String> {
    if aur.conflicts.is_empty() {
        vec![name.to_string()]
    } else {
        aur.conflicts.clone()
    }
}

/// Generate a `PKGBUILD` for the `-bin` package. Sources point at the GitHub
/// release tarballs; `package()` installs from `*/bin/` to match byt's staging
/// layout (`{name}-v{ver}-{triple}/bin/<exes>`).
#[allow(clippy::too_many_arguments)]
fn pkgbuild_body(
    name: &str,
    version: &str,
    pkgname: &str,
    aur: &Aur,
    url: &str,
    dl_base: &str,
    arches: &[AurArch],
    bins: &[String],
) -> String {
    use std::fmt::Write;
    let arch_list = arches
        .iter()
        .map(|a| format!("'{}'", a.carch))
        .collect::<Vec<_>>()
        .join(" ");
    let mut s = String::new();
    let _ = writeln!(s, "# Maintainer: {}", aur.maintainer);
    let _ = writeln!(s, "pkgname={pkgname}");
    let _ = writeln!(s, "pkgver={version}");
    let _ = writeln!(s, "pkgrel=1");
    let _ = writeln!(s, "pkgdesc=\"{}\"", aur.pkgdesc);
    let _ = writeln!(s, "arch=({arch_list})");
    let _ = writeln!(s, "url=\"{url}\"");
    let _ = writeln!(s, "license=('{}')", aur.license);
    let _ = writeln!(s, "provides=({})", quoted_list(&aur_provides(name, aur)));
    let _ = writeln!(s, "conflicts=({})", quoted_list(&aur_conflicts(name, aur)));
    for a in arches {
        let fname = archive_filename(name, version, a.triple);
        let _ = writeln!(
            s,
            "source_{c}=(\"{pkgname}-{version}-{c}.tar.gz::{dl_base}/{fname}\")",
            c = a.carch
        );
        let _ = writeln!(s, "sha256sums_{}=('{}')", a.carch, a.sha);
    }
    let _ = writeln!(s, "\npackage() {{");
    let _ = writeln!(s, "    cd \"$srcdir\"");
    for bin in bins {
        let _ = writeln!(
            s,
            "    install -Dm755 */bin/{bin} \"$pkgdir/usr/bin/{bin}\""
        );
    }
    let _ = writeln!(s, "}}");
    s
}

/// Generate `.SRCINFO` for the same package (deterministic — no `makepkg`).
fn srcinfo_body(
    name: &str,
    version: &str,
    pkgname: &str,
    aur: &Aur,
    url: &str,
    dl_base: &str,
    arches: &[AurArch],
) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "pkgbase = {pkgname}");
    let _ = writeln!(s, "\tpkgdesc = {}", aur.pkgdesc);
    let _ = writeln!(s, "\tpkgver = {version}");
    let _ = writeln!(s, "\tpkgrel = 1");
    let _ = writeln!(s, "\turl = {url}");
    for a in arches {
        let _ = writeln!(s, "\tarch = {}", a.carch);
    }
    let _ = writeln!(s, "\tlicense = {}", aur.license);
    for p in aur_provides(name, aur) {
        let _ = writeln!(s, "\tprovides = {p}");
    }
    for c in aur_conflicts(name, aur) {
        let _ = writeln!(s, "\tconflicts = {c}");
    }
    for a in arches {
        let fname = archive_filename(name, version, a.triple);
        let _ = writeln!(
            s,
            "\tsource_{c} = {pkgname}-{version}-{c}.tar.gz::{dl_base}/{fname}",
            c = a.carch
        );
        let _ = writeln!(s, "\tsha256sums_{} = {}", a.carch, a.sha);
    }
    let _ = writeln!(s, "\npkgname = {pkgname}");
    s
}

fn ensure_trailing_newline(s: &str) -> String {
    if s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

/// Run a git subcommand in `dir` with the AUR SSH command configured.
fn git_in(ctx: &RuntimeContext, dir: &Path, args: &[&str], ssh_cmd: &str) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).env("GIT_SSH_COMMAND", ssh_cmd);
    cmd.args(args);
    run(ctx, &mut cmd)
}

/// AUR: generate PKGBUILD + .SRCINFO for the release artifacts and push over
/// SSH (key from `$AUR_SSH_PRIVATE_KEY`). Inline in the release run so a failed
/// AUR publish fails the release (single log, synchronous — ADR-0021).
fn publish_aur(
    ctx: &RuntimeContext,
    cfg: &ReleaseSection,
    aur: &Aur,
    version: &str,
    repo: &str,
    dist: &Path,
) -> Result<()> {
    let pkgname = aur
        .pkgname
        .clone()
        .unwrap_or_else(|| format!("{}-bin", cfg.name));
    let url = aur
        .url
        .clone()
        .unwrap_or_else(|| format!("https://github.com/{repo}"));
    let dl_base = format!("https://github.com/{repo}/releases/download/v{version}");

    // Match each linux-gnu arch to its published sha256; skip arches not built.
    let checks = read_checksums(dist).unwrap_or_default();
    let mut arches = Vec::new();
    for &(carch, triple) in AUR_ARCHES {
        let fname = archive_filename(&cfg.name, version, triple);
        if let Some((_, sha)) = checks.iter().find(|(n, _)| n == &fname) {
            arches.push(AurArch {
                carch,
                triple,
                sha: sha.clone(),
            });
        }
    }
    if arches.is_empty() && !ctx.common.dry_run {
        bail!("no linux-gnu artifacts in checksums.txt to publish to AUR");
    }

    let bins = cfg.bins();
    let pkgbuild = pkgbuild_body(
        &cfg.name, version, &pkgname, aur, &url, &dl_base, &arches, &bins,
    );
    let srcinfo = srcinfo_body(&cfg.name, version, &pkgname, aur, &url, &dl_base, &arches);

    if ctx.common.dry_run {
        println!(
            "  [dry-run] AUR {pkgname} v{version}:\n{pkgbuild}\n----- .SRCINFO -----\n{srcinfo}"
        );
        return Ok(());
    }

    let key = std::env::var("AUR_SSH_PRIVATE_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .context("AUR_SSH_PRIVATE_KEY not set (needed to push to AUR)")?;
    let email = std::env::var("AUR_EMAIL").unwrap_or_else(|_| "dev@byteowlz.com".to_string());

    let work = std::env::temp_dir().join(format!("byt-aur-{pkgname}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let keyfile = work.join("aur_key");
    fs::write(&keyfile, ensure_trailing_newline(&key))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&keyfile, fs::Permissions::from_mode(0o600))?;
    }
    let known = work.join("known_hosts");
    fs::write(&known, "")?;
    let ssh_cmd = format!(
        "ssh -i {} -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile={}",
        keyfile.display(),
        known.display()
    );

    let repo_dir = work.join("pkg");
    let mut clone = Command::new("git");
    clone
        .arg("clone")
        .arg(format!("ssh://aur@aur.archlinux.org/{pkgname}.git"))
        .arg(&repo_dir)
        .env("GIT_SSH_COMMAND", &ssh_cmd);
    run(ctx, &mut clone)?;

    // AUR packages live on `master`. A fresh package clones as an empty repo
    // (unborn HEAD) and even an existing one can land detached; pin a branch so
    // the later `git push` has one to push. -B is a no-op on an already-correct
    // checkout and creates/repoints master otherwise.
    git_in(ctx, &repo_dir, &["checkout", "-B", "master"], &ssh_cmd)?;

    fs::write(repo_dir.join("PKGBUILD"), &pkgbuild)?;
    fs::write(repo_dir.join(".SRCINFO"), &srcinfo)?;

    git_in(ctx, &repo_dir, &["add", "PKGBUILD", ".SRCINFO"], &ssh_cmd)?;
    let clean = Command::new("git")
        .current_dir(&repo_dir)
        .args(["diff", "--cached", "--quiet"])
        .status()?
        .success();
    if clean {
        println!("  AUR: no changes for {pkgname} v{version}, skipping push");
        let _ = fs::remove_dir_all(&work);
        return Ok(());
    }
    git_in(
        ctx,
        &repo_dir,
        &["config", "user.name", "byteowlz"],
        &ssh_cmd,
    )?;
    git_in(ctx, &repo_dir, &["config", "user.email", &email], &ssh_cmd)?;
    git_in(
        ctx,
        &repo_dir,
        &["commit", "-m", &format!("Update to v{version}")],
        &ssh_cmd,
    )?;
    git_in(ctx, &repo_dir, &["push", "origin", "HEAD:master"], &ssh_cmd)?;
    println!("  published {pkgname} v{version} to AUR");
    let _ = fs::remove_dir_all(&work);
    Ok(())
}

/// Publish all configured downstream packages after the GitHub release exists.
fn publish_packages(
    ctx: &RuntimeContext,
    cfg: &ReleaseSection,
    version: &str,
    dist: &Path,
) -> Result<()> {
    if cfg.homebrew.is_none() && cfg.aur.is_none() {
        return Ok(());
    }
    let repo = resolve_repo()?;
    if let Some(hb) = &cfg.homebrew {
        publish_homebrew(ctx, hb, &cfg.name, version, &repo)?;
    }
    if let Some(aur) = &cfg.aur {
        publish_aur(ctx, cfg, aur, version, &repo, dist)?;
    }
    Ok(())
}

// ---- dispatch -------------------------------------------------------------

pub fn handle_release(ctx: &RuntimeContext, command: ReleaseCommand) -> Result<()> {
    let root = std::env::current_dir().context("getting current directory")?;
    let dist = root.join(DIST_DIR);
    match command {
        ReleaseCommand::Build { target, os } => {
            let cfg = load_config(&root)?;
            let version = resolve_version(&cfg, None, &root)?;
            let targets = select_targets(&cfg, target, os.as_deref());
            for triple in targets {
                build_target(ctx, &cfg, &root, &version, &triple, &dist)?;
            }
            Ok(())
        }
        ReleaseCommand::Targets { os } => {
            let cfg = load_config(&root)?;
            for triple in select_targets(&cfg, None, os.as_deref()) {
                println!("{triple}");
            }
            Ok(())
        }
        ReleaseCommand::Checksums => generate_checksums(ctx, &dist),
        ReleaseCommand::Publish { tag } => {
            let cfg = load_config(&root)?;
            let version = resolve_version(&cfg, tag.as_deref(), &root)?;
            publish(ctx, &cfg, &version, tag.as_deref(), &dist)
        }
        ReleaseCommand::Verify { path } => verify(&path),
        ReleaseCommand::Run { tag } => {
            let cfg = load_config(&root)?;
            let version = resolve_version(&cfg, tag.as_deref(), &root)?;
            for triple in cfg.targets() {
                build_target(ctx, &cfg, &root, &version, &triple, &dist)?;
            }
            generate_checksums(ctx, &dist)?;
            publish(ctx, &cfg, &version, tag.as_deref(), &dist)?;
            publish_packages(ctx, &cfg, &version, &dist)
        }
        ReleaseCommand::Packages { tag } => {
            let cfg = load_config(&root)?;
            let version = resolve_version(&cfg, tag.as_deref(), &root)?;
            publish_packages(ctx, &cfg, &version, &dist)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_and_staging_names_match_spec() {
        assert_eq!(
            archive_filename("mmry", "0.11.0", "x86_64-unknown-linux-gnu"),
            "mmry-v0.11.0-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            staging_dirname("mmry", "0.11.0", "x86_64-unknown-linux-gnu"),
            "mmry-v0.11.0-x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn zig_build_target_floors_linux_gnu_only() {
        assert_eq!(
            zig_build_target("x86_64-unknown-linux-gnu"),
            "x86_64-unknown-linux-gnu.2.28"
        );
        assert_eq!(
            zig_build_target("aarch64-unknown-linux-gnu"),
            "aarch64-unknown-linux-gnu.2.28"
        );
        assert_eq!(
            zig_build_target("aarch64-apple-darwin"),
            "aarch64-apple-darwin"
        );
        assert_eq!(
            zig_build_target("x86_64-unknown-linux-musl"),
            "x86_64-unknown-linux-musl"
        );
    }

    #[test]
    fn go_target_maps_triples() {
        assert_eq!(
            go_target("x86_64-unknown-linux-gnu").unwrap(),
            ("linux", "amd64")
        );
        assert_eq!(
            go_target("aarch64-unknown-linux-gnu").unwrap(),
            ("linux", "arm64")
        );
        assert_eq!(
            go_target("aarch64-apple-darwin").unwrap(),
            ("darwin", "arm64")
        );
        assert_eq!(
            go_target("x86_64-pc-windows-msvc").unwrap(),
            ("windows", "amd64")
        );
        assert!(go_target("mips-unknown-linux-gnu").is_err());
    }

    #[test]
    fn checksums_body_is_sorted_and_formatted() {
        let body = checksums_body(&[
            ("b.tar.gz".into(), "22".into()),
            ("a.tar.gz".into(), "11".into()),
        ]);
        assert_eq!(body, "11  a.tar.gz\n22  b.tar.gz\n");
    }

    #[test]
    fn sha256_hex_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn version_parsing() {
        assert_eq!(strip_v("v1.2.3"), "1.2.3");
        assert_eq!(strip_v("1.2.3"), "1.2.3");
        let cargo = "[package]\nname = \"x\"\nversion = \"0.9.1\"\n\n[dependencies]\nfoo = \"1\"\n";
        assert_eq!(parse_cargo_version(cargo).as_deref(), Some("0.9.1"));
        assert_eq!(
            parse_cargo_version("[dependencies]\nversion = \"9\"\n"),
            None
        );
    }

    #[test]
    fn verify_roundtrips_against_generated_checksums() {
        let dir = std::env::temp_dir().join(format!("byt-release-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let art = dir.join("x-v1.0.0-x86_64-unknown-linux-gnu.tar.gz");
        fs::write(&art, b"payload-bytes").unwrap();
        let sha = sha256_file(&art).unwrap();
        let body = checksums_body(&[("x-v1.0.0-x86_64-unknown-linux-gnu.tar.gz".into(), sha)]);
        fs::write(dir.join("checksums.txt"), body).unwrap();
        verify(&dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn os_of_and_select_targets_filter_by_os() {
        assert_eq!(os_of("x86_64-unknown-linux-gnu"), "linux");
        assert_eq!(os_of("aarch64-apple-darwin"), "macos");
        assert_eq!(os_of("x86_64-pc-windows-msvc"), "windows");
        let cfg = ReleaseSection {
            name: "x".into(),
            lang: Lang::Rust,
            bins: vec![],
            version: None,
            targets: vec![
                "x86_64-unknown-linux-gnu".into(),
                "aarch64-unknown-linux-gnu".into(),
                "aarch64-apple-darwin".into(),
            ],
            extra: vec![],
            sign: false,
            homebrew: None,
            aur: None,
        };
        assert_eq!(
            select_targets(&cfg, None, Some("linux")),
            vec![
                "x86_64-unknown-linux-gnu".to_string(),
                "aarch64-unknown-linux-gnu".to_string()
            ]
        );
        assert_eq!(
            select_targets(&cfg, None, Some("macos")),
            vec!["aarch64-apple-darwin".to_string()]
        );
        assert_eq!(
            select_targets(&cfg, Some("custom".into()), Some("linux")),
            vec!["custom".to_string()]
        );
    }

    fn sample_aur() -> Aur {
        Aur {
            pkgname: Some("trx-bin".into()),
            pkgdesc: "Minimal git-backed issue tracker".into(),
            license: "MIT".into(),
            url: None,
            provides: vec!["trx".into()],
            conflicts: vec!["trx".into(), "trx-git".into()],
            maintainer: "byteowlz <dev@byteowlz.com>".into(),
        }
    }

    #[test]
    fn pkgbuild_matches_bin_layout_and_arches() {
        let arches = vec![AurArch {
            carch: "x86_64",
            triple: "x86_64-unknown-linux-gnu",
            sha: "deadbeef".into(),
        }];
        let body = pkgbuild_body(
            "trx",
            "0.6.3",
            "trx-bin",
            &sample_aur(),
            "https://github.com/byteowlz/trx",
            "https://github.com/byteowlz/trx/releases/download/v0.6.3",
            &arches,
            &["trx".into(), "trx-tui".into()],
        );
        assert!(body.contains("pkgname=trx-bin"));
        assert!(body.contains("pkgver=0.6.3"));
        assert!(body.contains("arch=('x86_64')"));
        assert!(body.contains("conflicts=('trx' 'trx-git')"));
        // source renames the download but points at the spec artifact name
        assert!(body.contains(
            "source_x86_64=(\"trx-bin-0.6.3-x86_64.tar.gz::https://github.com/byteowlz/trx/releases/download/v0.6.3/trx-v0.6.3-x86_64-unknown-linux-gnu.tar.gz\")"
        ));
        assert!(body.contains("sha256sums_x86_64=('deadbeef')"));
        // installs from */bin to match byt's staging layout
        assert!(body.contains("install -Dm755 */bin/trx \"$pkgdir/usr/bin/trx\""));
        assert!(body.contains("install -Dm755 */bin/trx-tui \"$pkgdir/usr/bin/trx-tui\""));
    }

    #[test]
    fn srcinfo_lists_each_arch_and_provide() {
        let arches = vec![
            AurArch {
                carch: "x86_64",
                triple: "x86_64-unknown-linux-gnu",
                sha: "aa".into(),
            },
            AurArch {
                carch: "aarch64",
                triple: "aarch64-unknown-linux-gnu",
                sha: "bb".into(),
            },
        ];
        let body = srcinfo_body(
            "trx",
            "0.6.3",
            "trx-bin",
            &sample_aur(),
            "https://github.com/byteowlz/trx",
            "https://github.com/byteowlz/trx/releases/download/v0.6.3",
            &arches,
        );
        assert!(body.starts_with("pkgbase = trx-bin\n"));
        assert!(body.contains("\tarch = x86_64\n"));
        assert!(body.contains("\tarch = aarch64\n"));
        assert!(body.contains("\tprovides = trx\n"));
        assert!(body.contains("\tsha256sums_aarch64 = bb\n"));
        assert!(body.trim_end().ends_with("pkgname = trx-bin"));
    }
}
