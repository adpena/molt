# Packaging & Release Notes

This folder holds release assets, install scripts, and packaging templates.

## Layout

- `install.sh` / `install.ps1`: end-user installers (download + PATH setup).
- `INSTALL.md`: bundled in release artifacts as offline install notes.
- `templates/`: boilerplate for Homebrew, Scoop, and Winget.
- `../config/release_supply_chain.toml`: sole repository, target-matrix, and pinned-download
  authority used by release execution.

## Release workflow

`PACKAGING.md` defines the executable candidate → verify → attest → promote
authority. Tag the exact version from `pyproject.toml`; every target must prove
reproducibility and clean-consumer native execution before the one protected
promotion job can make a draft GitHub Release public.

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

Use `tools/release/` helpers to render package-manager projections from the
signed release manifest. They do not rebuild, rehash, or republish artifacts.

### Manifest rendering

After a release, download `release_manifest.json` and run:

```bash
python3 tools/release/update_manifests.py release_manifest.json
```

Rendered files land in `packaging/out/` for copy/paste into external repos.
