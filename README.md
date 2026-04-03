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
