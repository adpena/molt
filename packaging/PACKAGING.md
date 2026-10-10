# Molt release authority

Molt has one release pipeline: `.github/workflows/release.yml`. A release is an
existing exact `v<project.version>` tag at a landed source revision. Dispatch the
workflow from that same tag, with an existing unpublished draft containing only
`molt-release-exit-<source_sha>.zip`. Tag pushes do not publish releases. The
workflow rejects a branch dispatch, a dirty tree, duplicate drafts, moved tags,
or a source different from the workflow's signed GitHub identity.

Artifact reproducibility and the native/WASM installation smoke tests are necessary,
not sufficient, for semantic release acceptance. The
[initial-release contract](../ROADMAP.md#first-release-milestone) requires
current-revision end-to-end evidence for the declared verified subset on LLVM,
native and WASM, including Python API, C-API/ABI and ecosystem consumers across
every advertised version/OS/architecture/backend/profile cell. Missing, skipped or
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

Candidate wiring must run both independent native generations and both runtime
inventories, and pass their four roots to the assembler. The two invocations
establish build independence; equal receipts alone cannot distinguish a copied
secondary output from a second build. The structural pipeline below defines
this complete contract.
The verified-subset policy requires LLVM, native and WASM with both `dev` and
`release` guest profiles. Declared coordinates do not establish passing coverage.
Execution receipts bind source, reference/toolchain/CI identities and selected
profile labels, but omit the actual Molt compiler digest/profile/features and
selected runtime cell or generation identity. Candidate smoke execution cannot
replace that semantic chain. Bind observed build diagnostics and runtime
inventories through verified-subset receipts and candidate admission before claiming that
the semantically qualified compiler and runtime are the ones shipped.

Wheel production currently yields release assets; no PyPI upload stage is
wired. Registry publication, package-manager availability and native OS binary
signing/notarization need their own delivery evidence. Semantic H0
authentication and supply-chain attestations remain distinct mandatory gates.

Runtime-cell materialization and verified-subset execution also need measured
campaign capacity within their current job budgets (ninety minutes and six
hours respectively). This is a scheduling and product-latency obligation, not
permission to omit cells or raise every timeout without diagnosis.

## Structural pipeline

1. `config/release_targets.toml` generates the release target matrix;
   `config/release_supply_chain.toml` owns downloaded tool URL, digest, and size
   pins. Admission requires the complete source-bound E1-E4 evidence closure.
2. One Linux job builds the pure-Python wheel. It builds twice from independent
   `git archive` exports and admits exactly one byte-identical wheel.
3. Every target independently builds `molt-worker`, the production compiler and
   the native `molt` launcher twice through `tools/release/native_build.py`.
   `build_compiler.py` is its public entry point. Each invocation materializes
   the exact Git snapshot, uses a fresh Cargo home and target directory, pins
   the native compiler/linker executables, strips ambient build overrides and
   checks source and tool identities again after building. The worker uses
   `release-output`; the compiler and launcher use the independent `release`
   profile. Each output publishes its three binaries and `native-build.json`
   atomically; an existing output cannot be reused or replaced. Candidate
   admission requires different output roots, equal location-neutral receipts
   (source inventory, epoch, target, profiles, features, native tool bytes and
   the admitted LLVM SDK input identity),
   matching binary architectures, and byte identity for all three binaries.
   Build Python uses the existing runtime identity admission and is checked
   again before publication. The installed consumer binds its worker bytes and
   shipped source inventory back to the admitted native receipt. Native-build
   receipt v3 adds the pinned LLVM release, policy-source digest, exact static
   archive closure, configuration executable and captured SDK resource identity.
   Source and release compiler builds share the same SDK admission owner. The
   production feature tuple includes `llvm` with `llvm22-1-force-static`; keeping
   the base `llvm22-1` feature preserves the existing pin parser. This statically
   links LLVM, while system C/C++ dependencies remain subject to platform binary
   compatibility admission. Installed consumers do not need an LLVM SDK.
   Full-SDK setup uses the existing package provisioner on Linux and the pinned
   source bootstrap on macOS and Windows, including their existing resource and
   tool prerequisites. Provisioning does not waive final release evidence.
   Darwin builds pin `DEVELOPER_DIR`, use actual xcrun-selected tools, retain
   `SDKROOT`, and record SDK metadata, version and deployment target. Windows
   builds retain the activated Visual Studio installation and SDK environment;
   LLVM's ATL requirement is enforced only by the LLVM bootstrap. Rust channel
   admission reads the original `rust-toolchain.toml` Git blob with replacement
   objects disabled. `--build-root` selects a configuration-free build parent;
   its default is `RUNNER_TEMP` or the system temporary root, and a rejection
   names the interfering Cargo configuration. The narrowed tool search requires
   native CMake and Ninja, plus NASM on Windows x86_64 for aws-lc-sys. Missing
   dependencies fail before building and must be provisioned explicitly.
   `tools/release/runtime_cells.py` twice materializes the tagged commit with the
   canonical Git snapshot and produces every runtime cell the guest surface can
   select from the eleven logical lanes in `config/release_acceptance_matrix.toml`.
   The shipped reader owns validation and projects four native runtime profiles
   (shared by native and LLVM) and three WASM profiles through existing cell keys
   (profile x stdlib tier x source-extension loader; WASM hosted SIMD or
   freestanding, with the full CPython C-API export surface). Both inventories
   must match byte for byte and carry the snapshot's Git identity. Each complete
   inventory and its cells are validated in a private sibling directory, then
   published atomically without replacing an existing output. A failed build
   or admission leaves no partial distribution at the requested output path.
   The bundle
   ships them under `runtime/<cell-id>/`, declared in
   `release-compiler-source.json`. `COMPILER_BUNDLE_DIRECTORIES` in
   `molt.compiler_distribution` owns the top-level directory projection; the
   bundle builder checks it and the Homebrew renderer consumes it. Installers
   retain whole directories, including hidden source inputs and every runtime
   cell. Homebrew binds the current frontend to Python 3.14 and its package test
   compiles and executes an existing differential guest. This does not replace
   the complete installed native/WASM/version/profile campaign.
   Installed compilation selects, admits and
   retains only those cells and never runs Cargo. The same assembled tree is
   projected into a `py3-none-<platform>` wheel for pip. Its tag is derived
   from the shipped binaries by the exact-pinned audit tools: auditwheel for
   Linux (library closure, GLIBC/GLIBCXX/CXXABI versions, ISA level) and
   delocate for macOS (system-only dylibs, minimum deployment target); Windows
   tags are exact. The written wheel is re-audited, and the consumer audits it
   and every program it links from shipped runtime cells. An unsatisfied policy
   fails with the tool's evidence; nothing is repaired or re-tagged.
4. `tools/release/release_authority.py candidate` creates deterministic Molt and
   worker archives twice, compares them, and emits a target candidate receipt.
5. `tools/release/verify_consumer.py` extracts that immutable candidate into a
   clean temporary root, explicitly authorizes its private CLI dependencies, and
   runs the shipped launcher for every Python version
   in the candidate source's verified-subset policy. Each coordinate selects its
   exact reference interpreter and explicit guest Python semantics, with Cargo,
   rustc and rustup unavailable. It verifies the shipped runtime cells equal the
   candidate and the derived policy, then executes all eleven logical lanes for
   each declared reference Python. The platform wheel is installed into a separate
   environment and uses that same eleven-lane producer and receiver for the first
   reference Python. Each lane binds backend, target, guest profile, runtime Cargo
   profile and compiler Cargo profile. Native and LLVM use explicit
   `molt build --target native --backend cranelift|llvm`, followed by direct execution
   of the requested executable. Each WASM lane uses public `molt run --target wasm`, which
   performs its own linked build and runs it on the Node host. The guest
   receives a flag-shaped argument and an argument containing a space, and its
   stdout must equal a fixed literal that CPython reproduces. Every cell writes
   into its own directory. All cells must use the same production compiler,
   read from each build's own diagnostics, and the same native launcher. The
   source-bound consumer receipt binds observed interpreter/host identities, the
   guest source digest, public commands, native executables, the linked module
   named by each WASM execution manifest, and outputs to the exact candidate.
   The separate worker archive is extracted and executed as its sole command
   owner; the compiler bundle does not contain another copy. Installation is
   private to the consumer; uninstall checks prove no ambient Molt import or
   console script remains. All 44 logical products (eleven per declared Python
   and eleven from pip), including distinct native and LLVM products, are then
   replayed from one sealed Linux root after removing
   the installed owners. The root contains only those bound products, the
   source-bound Node runner closure, the canonical native supervisor, and loader,
   library and Node bytes extracted from exact pinned archives. Reference/build
   CPython remains outside this root. No package scripts or package installation
   run in it. A missing archive, unsupported filesystem adapter, missing engine
   capability, changed input, unexpected executable, incomplete process closure
   or wrong output prevents admission; there is no host replay fallback.

   `--execution-archive-cache` names explicitly provisioned inputs from
   `config/release_execution_roots.toml` and the existing Node tool-release pins.
   The explicit development command `python -m tools.release.provision_execution_archives
   --target <release-target-id> --execution-archive-cache <cache>` populates that
   cache through the same pinned archive transfer owner used by tool provisioning.
   CI runs it before candidate builds and passes the identical cache to verification.
   Transfers are bounded by the source size, restricted to admitted HTTPS origins,
   and published only after exact digest validation; no package scripts, payload
   executables or Docker pulls run during this step. All five cache inputs (four Debian providers and Node) then
   pass the consumer's same stable-descriptor archive reader, followed by the
   shared ELF dependency closure audit over the admitted payload bytes. Unsupported platform
   adapters fail this preflight rather than silently selecting a Linux payload.
   The verifier never fetches missing inputs or pulls a Docker image. It imports
   the exact retained root tar through `tools/cross_run.py`, checks the resulting
   uncompressed layer digest, and creates a fresh read-only, offline, private
   namespace container for each cell. The local Linux Docker engine and runc
   are execution providers whose identities and effective settings are retained.
   Their private proc/dev/sys mounts, bounded tmpfs mounts and generated
   `/etc/hosts`, `/etc/hostname` and `/etc/resolv.conf` are explicit provider
   inputs; host directories and Docker sockets are not guest mounts. Guest PATH
   and HOME name absent directories; loader, Python and Node selectors are absent.

   Native receipt/event export is bounded and verified through the one native
   supervisor, including COMPLETE, successful root exit and closed accounting.
   Native and WASM staging retains the identities of the actual copied bytes
   through root sealing. Manifest decoding, verified loader assets, tar members
   and ZIP extraction remain bound to their consumed bytes. The receiver also
   joins receipt and policy captures to the native verifier's consumed-input
   digests; local read-only mode alone is not an immutability claim.
   Raw capture, policy, provider identities, pinned archives, root bytes and every
   receipt/event stream are retained in a candidate-specific consumer evidence
   ZIP and covered by the release manifest/checksum/SBOM/attestation projection.
   Receiver admission rechecks those retained bytes and never executes guests.
   This installed smoke closure does not prove artifact reproducibility or the
   full verified subset. Linux x86_64/aarch64 are implemented source paths pending
   actual engine, ptrace and native/WASM qualification; macOS and Windows have no
   admitted filesystem adapter and therefore cannot pass this release gate yet.
   Public schema declarations must move with their producer versions.

6. Only after every target passes does one index job create the collision-free
   release manifest, SHA256SUMS, and SPDX 2.3 SBOM, including the evidence ZIP and any
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
E4 structural receipts bind the Python audit interpreter by observed command
and base-executable bytes, implementation, exact version, and the canonical
`molt.python-runtime-closure.v5` inventory. That existing authority identifies
the loaded CPython runtime-library image, import roots, runtime files, and native
dependency closure; a launcher hash alone cannot establish this identity.
The observation is captured before the audit and rechecked before publication.
Unavailable or changed runtime closure prevents receipt publication. Producer
scripts and inspected source inputs are hashed separately. This audit-engine identity does not establish E1/E2 guest
compiler, runtime, or oracle toolchain identity; missing observations still make
the corresponding H0 predicate false.
E1 acceptance receipts record the actual acceptance-producer invocation and its
completion observation time. Their `execution_tools` remains explicitly null
until the actual compiler, runtime, and reference-oracle identity closure is
admitted by a typed authority; neither a version string nor the receipt writer's
Python identity can satisfy that missing H0 toolchain obligation.

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

### LLVM attribution and qualification

`vendor/llvm/LICENSE.TXT` retains the exact notice from the source-pinned LLVM
release. Bundle production copies it to `share/molt/LLVM-LICENSE.TXT`, and the
platform wheel projects the same tree. The SBOM projects one SDK package per
release host from the admitted native-build v3 input record, including upstream
source digest, static link closure and location-neutral SDK byte identity.
This uses the existing candidate admission and signing path; a feature list or
self-reported summary cannot replace native-build and installed-consumer receipts.
Consumer proof v8 and sealed replay v2 carry complete logical lane records.
The six-host native/LLVM/WASM execution and performance matrix must still be
qualified on the exact final candidate; the implemented delivery path does not
establish those results or resolve the separate release-evidence production cycle.
