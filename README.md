# byt

byteowlz meta-tool for cross-repo management and governance.

## Installation

```bash
just install
```

## Usage

```bash
# Catalog management
byt catalog refresh      # Scan repos, generate CATALOG.json
byt catalog show         # Display catalog
byt catalog list         # List repo names

# Current repo / active copy
byt current              # Detect repo from current dir and compare across machines
byt current mmry         # Show which machine likely has the active mmry copy
byt current mmry --json  # Machine-readable cross-machine status

# Governance
byt lint                 # Check compliance across repos
byt status               # Show repo status matrix

# Shared git hooks (byteowlz/githooks, always-current)
byt hooks install        # Pull latest hooks into machine cache + wire core.hooksPath
byt hooks status         # Show wired hooks path, hook files, gitleaks
byt hooks update         # Force-refresh hooks cache (no repo rewiring)

# Issue tracking
byt ready                # Govnr-level ready work

# Releases (ADR-0021/0022; see .github/workflows/release.yml)
byt release build --os linux   # Build configured linux targets
byt release targets            # Print configured target triples
byt release checksums          # Write dist/release/checksums.txt

# Public mirrors (private working repo -> public snapshot)
byt publish init         # Write publish.toml listing this repo's markdown
byt publish check        # Show what an export would change; leak scan; writes nothing
byt publish export       # Commit a snapshot into ~/byteowlz-public/<repo> (no push)
byt publish export --push [--prune-branches]
byt publish export --tag v1.2.3 --push   # Release: snapshot + tag, starts the public release workflow

# Knowledge
byt memory search "q"    # Search memories (via mmry)
byt memory add "content" # Add memory
```

## Public mirrors

A working repository can stay private (e.g. on Forgejo) with its agent
instructions, decision records and research, while its public GitHub repository
receives snapshots. `byt publish export` takes committed content only and:

- keeps markdown only when `publish.toml` lists it (`docs = [...]`); agent and
  context files (`AGENTS.md`, `CLAUDE.md`, `CONTEXT.md`), `publish.toml` itself
  and `.trx`/`.pi`/`.claude` are never exported;
- refuses on home paths, tailnet addresses, private keys, the local user name,
  `forbidden` terms and links in exported markdown to files that stay private;
  exact harmless matches go in `allow`;
- optionally runs `nym detect` too (`nym = true`, `nym_args`);
- commits one snapshot into `~/byteowlz-public/<repo>` (cloned from
  `public_remote` on first use, so history and tags continue) and pushes only
  with `--push`;
- publishes `main` only: it refuses a public checkout on another branch,
  reports other public branches, and deletes them with `--push --prune-branches`.
  Work branches live in the private repository.
- releases: tag the release commit in the private repository, then
  `byt publish export --tag <tag> --push`. The tag must point at the private
  HEAD; it is created on the snapshot and pushed after `main`, which starts the
  public release workflow. A public release tag is never moved.

## Release builder

`ghcr.io/byteowlz/byt` (built from this repo's `Dockerfile`) is the shared
builder image: every byteowlz repo's release workflow runs `byt release` inside
it. Linux-gnu targets go through `cargo zigbuild` with a glibc 2.28 floor.

Rust apps that link **system native libraries** (e.g. trnscrb's speakrs
diarization backend -> `openblas-system`) are supported: the image carries
`pkg-config` + `libopenblas-dev` for its native arch and, via Debian multiarch,
for arm64. `byt release` exports per-target env so the zig linker and pkg-config
resolve the *target's* library (`/usr/lib/<multiarch>-linux-gnu`), since zig cc
searches only its own sysroot. The built binary keeps the glibc floor and gains
a **runtime dependency** on the deployment host's library (`libopenblas.so.0`) —
apps must document that dependency (AUR `depends`, README requirements).

Mac targets build natively on the runner chosen by the reusable workflow's
`macos_runner` input. Callers may opt in to build-time Homebrew formulae with
`macos_brew_deps` (space-separated names, e.g. `openblas`). The archive does
not bundle their dylibs; apps must document runtime dependencies separately.

## Integration

byt integrates with:

- `trx` for issue tracking
- `mmry` for memory management

## License

MIT
