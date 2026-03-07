# Molt Package Distribution Plan

## Naming Convention: `molt-lang` everywhere

| Platform | Package Name | CLI Command | Status |
|----------|-------------|-------------|--------|
| PyPI | `molt-lang` | `molt` | Ready (pyproject.toml updated) |
| crates.io | `molt-lang-backend` etc. | N/A (library crates) | Needs Cargo.toml rename |
| Homebrew | `molt-lang` | `molt` | Formula draft at packaging/homebrew/ |
| GitHub | `adpena/molt` | N/A | Current |

## PyPI (`molt-lang`)

```bash
# Build
uv build

# Test locally
uv pip install dist/molt_lang-0.1.0-py3-none-any.whl

# Publish
uv publish --token $PYPI_TOKEN
```

The PyPI package includes the Python frontend only. Users need the Rust
backend binary separately (via Homebrew or cargo install).

**Install flow:**
```bash
pip install molt-lang      # Python frontend + CLI
brew install molt-lang     # Rust backend binary
molt build hello.py        # Works!
```

## Homebrew (`molt-lang`)

Two options:
1. **Tap:** `brew tap adpena/molt && brew install molt-lang`
2. **Core:** Submit to homebrew-core (requires popularity threshold)

Start with a tap. Formula at `packaging/homebrew/molt-lang.rb`.

**Setup tap:**
```bash
# Create repo: adpena/homebrew-molt
gh repo create adpena/homebrew-molt --public
cp packaging/homebrew/molt-lang.rb .
git add . && git commit -m "Add molt-lang formula"
git push
```

## crates.io (`molt-lang-*`)

Current Rust crate names (`molt-backend`, `molt-runtime`, etc.) conflict
with the `molt` TCL interpreter crate (0.3.1 on crates.io).

**Rename plan:**
| Current | New (crates.io) |
|---------|-----------------|
| `molt-backend` | `molt-lang-backend` |
| `molt-runtime` | `molt-lang-runtime` |
| `molt-python` | `molt-lang-python` |
| `molt-obj-model` | `molt-lang-obj-model` |
| `molt-db` | `molt-lang-db` |
| `molt-wasm-host` | `molt-lang-wasm-host` |
| `molt-worker` | `molt-lang-worker` |

This rename touches Cargo.toml in 7 crates plus all cross-references.
Do it in a single commit before first crates.io publish.

## Version Strategy

- Start at `0.1.0` (alpha)
- Follow semver: `0.x.y` until stable
- Tag releases as `v0.1.0` etc.
- PyPI and crates.io versions stay in sync
