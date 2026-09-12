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

# Issue tracking
byt ready                # Govnr-level ready work

# Knowledge
byt memory search "q"    # Search memories (via mmry)
byt memory add "content" # Add memory
```

## Integration

byt integrates with:

- `trx` for issue tracking
- `mmry` for memory management

## License

MIT
