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
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::RuntimeContext;

/// Staging/output directory, relative to the repo root.
const DIST_DIR: &str = "dist/release";
/// Per-repo release config consumed by `byt release`.
const CONFIG_FILE: &str = "byt.release.toml";
const DEFAULT_EXTRA: &[&str] = &["LICENSE", "README.md"];
const DEFAULT_TARGETS: &[&str] = &["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"];

#[derive(Debug, clap::Subcommand)]
pub enum ReleaseCommand {
    /// Compile + stage + archive one target (or all configured targets).
    Build {
        /// Build a single target triple instead of all configured ones.
        #[arg(long)]
        target: Option<String>,
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
    /// Full pipeline: build all targets + checksums + publish.
    Run {
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
}

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

/// Resolve the release version: explicit `--tag` → `$GITHUB_REF_NAME` →
/// `[release].version` → `Cargo.toml`. All forms are normalized without a `v`.
fn resolve_version(cfg: &ReleaseSection, tag: Option<&str>, root: &Path) -> Result<String> {
    if let Some(tag) = tag {
        return Ok(strip_v(tag));
    }
    if let Ok(reff) = std::env::var("GITHUB_REF_NAME")
        && !reff.is_empty()
    {
        return Ok(strip_v(&reff));
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
    bail!("could not resolve version: pass --tag, set [release].version, or $GITHUB_REF_NAME")
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

fn compile_rust(ctx: &RuntimeContext, root: &Path, triple: &str) -> Result<()> {
    // cargo-zigbuild gives reliable cross-compilation from a Linux host.
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root)
        .arg("zigbuild")
        .arg("--release")
        .arg("--target")
        .arg(triple);
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

// ---- dispatch -------------------------------------------------------------

pub fn handle_release(ctx: &RuntimeContext, command: ReleaseCommand) -> Result<()> {
    let root = std::env::current_dir().context("getting current directory")?;
    let dist = root.join(DIST_DIR);
    match command {
        ReleaseCommand::Build { target } => {
            let cfg = load_config(&root)?;
            let version = resolve_version(&cfg, None, &root)?;
            let targets = match target {
                Some(t) => vec![t],
                None => cfg.targets(),
            };
            for triple in targets {
                build_target(ctx, &cfg, &root, &version, &triple, &dist)?;
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
            publish(ctx, &cfg, &version, tag.as_deref(), &dist)
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
}
