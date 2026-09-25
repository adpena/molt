# Molt release authority

Molt has one release pipeline: `.github/workflows/release.yml`. A release is an
existing exact `v<project.version>` tag at a landed source revision. Dispatch the
workflow from that same tag, with an existing unpublished draft containing only
`molt-release-exit-<source_sha>.zip`. Tag pushes do not publish releases. The
workflow rejects a branch dispatch, a dirty tree, duplicate drafts, moved tags,
or a source different from the workflow's signed GitHub identity.

Artifact reproducibility and the native installation smoke test are necessary,
not sufficient, for semantic release acceptance. The
[initial-release contract](../ROADMAP.md#first-release-milestone) requires
current-revision end-to-end evidence for the declared verified subset on native
and WASM, including Python API, C-API/ABI and ecosystem consumers across every
advertised version/OS/architecture/backend/profile cell. Missing, skipped or
diagnostic-only cells do not count as passes. The packaging workflow below does
not currently establish that complete acceptance matrix by itself.

The semantic evidence authority is `tools/release_exit_gate.py`, which verifies
the source-addressed E1-E4 bundle. Every release, including `v0.0.1`, must pass
that gate. The original canonical ZIP is admitted before builds, pinned by its
digest, reverified in the signing checkout, and published unchanged. Stable
`v1.0` and later releases additionally require a green H0 phase exit, projected
from those same receipts and cryptographically authenticated in the signing job.
The current verified-subset
coordinates and scientific witness also do not establish complete public/C-API
coverage, browser execution, or determinism across every advertised profile.
These remain release blockers, not implicit passes from packaging success.

## Structural pipeline

1. `config/release_targets.toml` generates the release target matrix;
   `config/release_supply_chain.toml` owns downloaded tool URL, digest, and size
   pins. Admission requires the complete source-bound E1-E4 evidence closure.
2. One Linux job builds the pure-Python wheel. It builds twice from independent
   `git archive` exports and admits exactly one byte-identical wheel.
3. Every target independently builds `molt-worker`, the production compiler and
   the native `molt` launcher twice with locked Cargo inputs. The worker uses
   `release-output`; the compiler and launcher use the independent `release`
   profile, enforced by `build_compiler.py`. Byte identity is mandatory for all
   three binaries.
4. `tools/release/release_authority.py candidate` creates deterministic Molt and
   worker archives twice, compares them, and emits a target candidate receipt.
5. `tools/release/verify_consumer.py` extracts that immutable candidate into a
   clean temporary root, explicitly authorizes its private CLI dependencies, and
   runs the shipped launcher for every Python version
   in the candidate source's verified-subset policy. Each cell selects its exact
   reference interpreter and explicit guest Python semantics, then builds and
   executes a standalone native program with both `dev` and `release` profiles.
   All cells must use the same production compiler and native launcher. The
   source-bound consumer receipt binds observed interpreter/host identities,
   executable identities, build/run commands and outputs to the exact candidate.
   The separate worker archive is extracted and executed as its sole command
   owner; the compiler bundle does not contain another copy. Installation is
   private to the consumer; uninstall checks prove no ambient
   import or console script remains. Schema versions are checked against their
   producers by the public-contract gate rather than restated here.
6. Only after every target passes does one index job create the collision-free
   v3 manifest, SHA256SUMS, and SPDX 2.3 SBOM, including the evidence ZIP and any
   required H0 manifest and signature bundle. GitHub's pinned attestation action
   signs SLSA provenance and the SBOM using a keyless Sigstore OIDC certificate.
7. One protected promotion job rechecks the pinned draft id, original evidence
   asset id and source tag. Uploads and downloads use these numeric identities,
   not another tag lookup. It retains the original evidence asset, admits
   already-staged files only when their bytes match, and uploads missing files
   without clobbering or deleting assets. After verifying the exact signed asset
   set, the canonical `promote-release` transaction flips `draft=false` once and
   checks the published assets again. One private payload snapshot is
   authenticated for the entire transaction. Failed pre-publication verification
   leaves the draft unpublished; rerunning that promotion job resumes matching
   staged assets. If publication already succeeded, a rerun verifies the public
   release without modifying it. Resume requires the original signed Actions
   artifact, retained for 14 days; a fresh dispatch requires an evidence-only
   draft. A failed upload left in an incomplete state is reported by asset ID
   and requires an operator audit before manual removal; no cleanup is automatic.

E3 receipt signatures must come from this repository's `verified-subset.yml` at
the release source digest, on GitHub-hosted runners. H0 and release signatures
must come from `release.yml` under the same constraints. Subject digests alone
are not signature authentication. E1, E2 and E4 are currently typed,
source-bound receipts, not independently signed execution attestations; release
signing attests their verified contents, not an independent rerun.

H0 is a separate signed projection, outside the E1-E4 bundle's closed file
inventory. `phase_exit_manifest prepare` emits the canonical unsigned subject
only after its semantic predicates pass; `seal` attaches a signature to those
exact bytes and rechecks the predicates. An unsigned subject is never a green
phase exit. The release index verifies the signature cryptographically.

The `release-production` environment should allow only release tags. Both draft
admission and promotion use that protected environment: GitHub requires push
access to list unpublished drafts. Neither step creates a release or a tag.

## Stage and dispatch

Assemble the complete same-source bundle with `tools/release_exit_gate.py`, then
create its canonical ZIP using `uv run --python 3.12 python -m
tools.release.release_authority archive-exit --manifest <bundle>/release-exit.json
--source-sha <commit> --source-date-epoch <commit-epoch> --output
molt-release-exit-<commit>.zip`. The archive command verifies the bytes it actually
packs; it cannot turn failed or missing evidence into acceptance.

Create the exact tag and a draft containing only that ZIP. Dispatch with
`gh workflow run release.yml --ref v<version> -f version=v<version>`.
An interrupted upload may leave extra assets on the draft; audit and restore
the exact evidence-only draft before restarting admission. Do not delete
evidence or replace an already published release to make admission pass.

Cloudflare and Modal deployments are separate release-event workflows with
protected environments. They cannot publish or mutate compiler release assets.

## Local structural checks

```bash
uv run --python 3.12 python -m tools.release.release_authority validate
uv run --python 3.12 python -m pytest -q tests/tools/test_release_supply_chain.py tests/tools/test_release_archive.py tests/tools/test_phase_exit_manifest.py
uv run --python 3.12 python tools/gen_proof_plan.py --check
```

The release workflow itself is the cross-platform executable proof. Local tests
exercise deterministic archive assembly, matrix/digest admission, path traversal
rejection, pinned download validation, and topology teeth without publishing.

## Package-manager projections

`release_manifest.json` remains the sole input to the Homebrew, Scoop, and Winget
template renderer:

```bash
uv run --python 3.12 python -m tools.release.update_manifests release_manifest.json
```

External package repositories consume the already-published manifest and never
recalculate artifact digests.
