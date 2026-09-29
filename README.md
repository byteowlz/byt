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

# Knowledge
byt memory search "q"    # Search memories (via mmry)
byt memory add "content" # Add memory
```

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
