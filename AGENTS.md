# byt - Byteowlz Meta-Tool

## Overview

`byt` is the meta-tool for managing the Byteowlz ecosystem. It provides cross-repository management, governance compliance checking, and integration with key tools (bd, mmry).

## Commands

| Command                         | Description                                              |
| ------------------------------- | -------------------------------------------------------- |
| `byt catalog refresh`           | Scan all repos and generate CATALOG.json                 |
| `byt catalog show`              | Display the current catalog                              |
| `byt catalog list`              | List all repository names                                |
| `byt catalog machines show`     | Show repos available on each machine                     |
| `byt catalog machines compare`  | Compare repo availability across machines                |
| `byt catalog machines missing`  | Show repos missing locally that exist on remotes         |
| `byt lint`                      | Check governance compliance (justfile, trx, AGENTS.md) |
| `byt status`           | Show repository status and compliance matrix             |
| `byt ready`            | Show ready work from govnr-level trx                   |
| `byt memory add`       | Add a memory (via mmry)                                  |
| `byt memory search`    | Search memories (via mmry)                               |
| `byt memory projects`  | List available memory stores                             |
| `byt sync status`      | Show memory sync state                                   |
| `byt sync push`        | Export memories to .sync/memories/                       |
| `byt sync pull`        | Import memories from .sync/memories/                     |

## Architecture

byt integrates with:

- **trx** - Issue tracking at govnr and repo levels
- **mmry** - Memory storage for architectural decisions

## Key Files

- `CATALOG.json` - Generated catalog of all repos with metadata
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

## Issue Tracking (trx)

```bash
trx ready              # Show unblocked issues
trx create "Title" -t task -p 2   # Create issue (types: bug/feature/task/epic/chore, priority: 0-4)
trx update <id> --status in_progress
trx close <id> -r "Done"
trx sync               # Commit .trx/ changes
```

Priorities: 0=critical, 1=high, 2=medium, 3=low, 4=backlog

## Memory System (byt/mmry)

Use `byt memory` to store and retrieve project knowledge. Memories auto-detect the current repo.

**Adding memories:**

```bash
byt memory add "Important decision or learning"              # Auto-detects current repo
byt memory add "Cross-repo architecture decision" --govnr    # Force govnr store
byt memory add "Specific insight" -c "architecture" -i 8     # With category and importance
```

**Searching memories:**

```bash
byt memory search "query"           # Search current repo's memories
byt memory search "query" --govnr   # Search cross-repo memories
byt memory search "query" --all     # Search ALL projects
```

**When to add memories:**

- Architecture decisions and their rationale
- Non-obvious solutions to tricky problems
- Integration patterns with other byteowlz repos
- Performance findings or benchmarks
- API contracts or breaking changes

**When to search memories:**

- Before starting work on a feature (check for prior decisions)
- When encountering unfamiliar code patterns
- When integrating with other repos (`byt memory search "query" --all`)
