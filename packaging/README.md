# Packaging & Release Notes

This folder holds release assets, install scripts, and packaging templates.

## Layout

- `install.sh` / `install.ps1`: end-user installers (download + PATH setup).
- `INSTALL.md`: bundled in release artifacts as offline install notes.
- `templates/`: boilerplate for Homebrew, Scoop, and Winget.
- `config.toml`: naming + repo metadata used by release helpers.

## Canonical namespace

- Canonical cross-registry package name: `molt-python`
- Cargo crate path in this repo: `runtime/molt-python`
- Cargo release version must start at `0.0.1`; `0.0.001` is invalid SemVer for Cargo
- `packaging/config.toml` `[ecosystem_names]` is the source of truth for the
  cross-registry target names we want to reserve/publish

## Release workflow (summary)

1. Tag the release `v0.0.001` (increment the thousandths place).
2. GitHub Actions builds artifacts for macOS/Linux/Windows and publishes a release.
3. Update external package repos (Homebrew/Scoop/Winget) using the templates.

## Packaging invariants

- The Molt toolchain itself may depend on local Python/Rust toolchains to build software,
  but binaries produced by `molt build` are expected to be standalone artifacts that run
  without a host Python installation.
- Shipped artifacts must not rely on hidden host-CPython fallback or a production bridge lane.
- Release packaging should minimize SmartScreen/Gatekeeper/quarantine friction through
  predictable artifact names, signatures/notarization where supported, and explicit checksums.

## External package repos

This repo only contains templates. You will need to push updates to:

- Homebrew tap: `adpena/homebrew-molt`
- Scoop bucket: `adpena/scoop-molt`
- Winget: submit manifest PRs via winget-pkgs

Use `tools/release/` helpers to generate updated manifests from the release
manifest (checksums + URLs).

### Manifest rendering

After a release, download `release_manifest.json` and run:

```bash
python3 tools/release/update_manifests.py release_manifest.json
```

Rendered files land in `packaging/out/` for copy/paste into external repos.
