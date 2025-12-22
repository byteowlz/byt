# byt - Byteowlz Meta-Tool

## Overview

`byt` is the meta-tool for managing the Byteowlz ecosystem. It provides cross-repository management, governance compliance checking, and integration with key tools (bv, cass, mmry).

## Commands

| Command | Description |
|---------|-------------|
| `byt catalog refresh` | Scan all repos and generate CATALOG.json |
| `byt catalog show` | Display the current catalog |
| `byt catalog list` | List all repository names |
| `byt lint` | Check governance compliance (justfile, beads, AGENTS.md) |
| `byt status` | Show repository status and compliance matrix |
| `byt ready` | Show ready work from govnr-level beads |
| `byt triage` | Cross-repo triage via bv workspace aggregation |
| `byt triage --next` | Get single top recommendation |
| `byt triage --refresh` | Regenerate workspace.yaml before triage |
| `byt search <query>` | Search agent conversation history (via cass) |
| `byt memory add` | Add a memory (via mmry) |
| `byt memory search` | Search memories (via mmry) |
| `byt memory projects` | List available memory stores |
| `byt sync status` | Show memory sync state |
| `byt sync push` | Export memories to .sync/memories/ |
| `byt sync pull` | Import memories from .sync/memories/ |

## Architecture

byt integrates with:
- **bd (beads)** - Issue tracking at govnr and repo levels
- **bv (beads_viewer)** - Cross-repo issue triage via workspace config
- **cass** - Agent conversation search across all sessions
- **mmry** - Memory storage for architectural decisions

## Key Files

- `CATALOG.json` - Generated catalog of all repos with metadata
- `.bv/workspace.yaml` - Auto-generated bv workspace config
- `.sync/memories/` - Cross-machine memory sync directory
- `.ignore` - Makes gitignored repos visible to AI tools

## Development

```bash
just build       # Debug build
just install     # Install to ~/.cargo/bin
just test        # Run tests
just clippy      # Lint
```

## Global Flags

- `--json` - Machine-readable JSON output
- `--workspace <PATH>` - Override workspace root
- `--dry-run` - Preview changes without writing
- `-v` / `--verbose` - Increase logging verbosity
