# Molt release authority

Molt has one release pipeline: `.github/workflows/release.yml`. A release is an
existing exact `v<project.version>` tag. The workflow does not publish from a
branch, a dirty tree, or an untagged manual checkout.

## Structural pipeline

1. `config/release_supply_chain.toml` emits the complete target matrix and owns
   downloaded tool URL, digest, and size pins.
2. One Linux job builds the pure-Python wheel. It builds twice from independent
   `git archive` exports and admits exactly one byte-identical wheel.
3. Every target independently builds `molt-worker` twice with locked Cargo input
   and the shipped `release-output` profile. Byte identity is mandatory.
4. `tools/release/release_authority.py candidate` creates deterministic Molt and
   worker archives twice, compares them, and emits a target candidate receipt.
5. `tools/release/verify_consumer.py` extracts that immutable candidate into a
   clean temporary root, installs its bundled wheel, invokes the CLI, compiles
   and executes a standalone native program, uninstalls Molt, and proves the
   import and console script are gone.
6. Only after every target passes does one index job create the collision-free
   manifest, SHA256SUMS, and SPDX 2.3 SBOM. GitHub's pinned attestation action
   signs SLSA provenance and the SBOM using a keyless Sigstore OIDC certificate.
7. One protected promotion job stages a draft GitHub Release, downloads every
   staged asset, byte-compares the complete set, and flips `draft=false` once.
   Failed staging is never public.

Cloudflare and Modal deployments are separate release-event workflows with
protected environments. They cannot publish or mutate compiler release assets.

## Local structural checks

```bash
python -m tools.release.release_authority validate
python -m pytest -q tests/tools/test_release_supply_chain.py
python tools/gen_proof_plan.py --check
```

The release workflow itself is the cross-platform executable proof. Local tests
exercise deterministic archive assembly, matrix/digest admission, path traversal
rejection, pinned download validation, and topology teeth without publishing.

## Package-manager projections

`release_manifest.json` remains the sole input to the Homebrew, Scoop, and Winget
template renderer:

```bash
python tools/release/update_manifests.py release_manifest.json
```

External package repositories consume the already-published manifest and never
recalculate artifact digests.
