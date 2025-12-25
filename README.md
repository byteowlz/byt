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

# Governance
byt lint                 # Check compliance across repos
byt status               # Show repo status matrix

# Issue tracking
byt ready                # Govnr-level ready work
byt triage               # Cross-repo triage (via bv)
byt triage --next        # Single top recommendation

# Knowledge
byt search "query"       # Search agent sessions (via cass)
byt memory search "q"    # Search memories (via mmry)
byt memory add "content" # Add memory
```

## Integration

byt integrates with:

- `bd` / `bv` for issue tracking
- `cass` for agent session search
- `mmry` for memory management

## License

MIT
