# v0.1 support matrix: one typed Tier 1 authority (HF-620)

Status: design, reviewed in two Goldfish rounds (section 9). After round 2 the release-line migration
(4.2) is cut from the HF-620 lane and specified as its own slice (section 11); the lane implements 4.1,
4.3, 4.4, 4.5 and 4.6. Owner decisions of 2026-11-05 bind it:

- "Both of the symposium's two recommendations are approved. But Python 3.15 belongs in there too."
- "All of the symposium's recommendations are approved. We must not lose our project to process."
- Approved recommendation: "Set Tier 1 to the cells the two flagship witnesses need: Django offload on
  native, and Falcon OCR on Workers." Every other cell is Tier 2: tested, not release-blocking. Tier 1
  widens only by owner decision. The release is v0.1; a v0.x bundle reports open rows, and the gate
  refuses PASS at major 1 or later while any row is open.

## 1. The Tier 1 cells (reviewed and approved)

The derivation, with file:line evidence and two Goldfish rounds, is the lane record
`logs/agents/lane-hf620-v01-claim/artifacts/tier1-cells.md` (revision 3). The integrator approved it
with one change: the Django witness's cell is the Molt-compiled native cell that spec 0600 names
("offloading one endpoint to a Molt-compiled worker"), not the current Rust-handler stub (HF-788).

| Witness | Build host | Python | Backend | Artifact target | Release lane (guest / runtime) | Execution |
| --- | --- | --- | --- | --- | --- | --- |
| Falcon OCR on Workers | macos-arm64 | 3.12 | wasm | `wasm32-wasip1`, linked | release / `wasm-release` | workerd (local Wrangler) |
| Falcon OCR on Workers | macos-arm64 | 3.15 | wasm | `wasm32-wasip1`, linked | release / `wasm-release` | workerd |
| Django offload | linux-x86_64 | 3.12 | native (Cranelift) | `x86_64-unknown-linux-gnu` | release / `release-output` | host |
| Django offload | linux-x86_64 | 3.15 | native (Cranelift) | `x86_64-unknown-linux-gnu` | release / `release-output` | host |

Every Tier 1 cell uses the GIL runtime. A 3.15 cell runs a 3.15 frontend with `--python-version 3.15`.

## 2. Problem, with evidence

The support matrix and the version rule have six homes, and they disagree.

| Home | What it says | Consumers |
| --- | --- | --- |
| `config/release_targets.toml` → generated `src/molt/release_matrix.py` | six release targets; Python = `target_python_policy` (3.12–3.14) | release tooling, toolchains, `public_contract_gate`, verified subset |
| `config/verified_subset.toml` + `src/molt/verified_subset.py` | the Cartesian product targets × Python × {llvm, native, wasm} × {dev, release}: 108 coordinates | release gate E3 roles, H0 phase exit, contract digest |
| `config/release_acceptance_matrix.toml` | backend × profile lanes; excluded experimental backends | `src/molt/release_lanes.py`, `tools/release_matrix_acceptance.py` |
| `tools/proof_plan.toml` `[[matrix_cell]]` | 26 CI cells; all Python 3.12; includes `linux-x86_64-py312-mlir-dev` | proof plan, CI workflows |
| Ledger: HF-620 row and "Release decision and exit scope" | 10-30 Tier 1 (four hosts, 3.12–3.15, LLVM, JIT, WebGPU); "the version number never relaxes the gate" | humans; integrator |
| `docs/spec/PUBLIC_CONTRACT_V1.md:106-115` | "Pre-1.0 tags … need no H0 … open findings … do not hold a pre-1.0 release" | humans |

None of them has a tier. The version rule conflicts: `tools/release_exit_gate.py:7-10` and
`_derive_status` (`:575-588`) hold every release on every open row, and `release_model.py:251` makes H0
mandatory in every manifest, while `PUBLIC_CONTRACT_V1.md:113-115` says a pre-1.0 release is not held.

## 3. Prior art

- **Rust platform tiers** (RFC 2803, "Target Tier Policy"): Tier 1 targets are named in one policy list;
  "Tier 1 targets … guaranteed to work" with CI gating; Tier 2 builds but tests are not release-blocking;
  promotion is a governed decision. Molt follows the same two-level shape and the owner-decision gate.
- **Go first-class ports** (`golang.org/wiki/PortingPolicy`): a first-class port's broken builder blocks
  a release; secondary ports do not.
- **CPython PEP 11**: tiers are named per platform triple, with explicit support owners.
- **Semver 2.0 §4**: "Major version zero (0.y.z) is for initial development … the public API should not be
  considered stable." Molt keeps the stable-contract hold for 1.x and reports open rows in 0.x.

All three keep the tier list in one declarative file that CI and the release process read. None keeps a
second hand-maintained copy.

## 4. Mechanism

### 4.1 One authority: `config/release_targets.toml`, schema v2

HF-620's exit text already names this authority (`molt.release_matrix`). Schema
`molt.release-targets.v2` adds a `[[tier1_cell]]` table:

```toml
[[tier1_cell]]
witness = "falcon-ocr-workers"   # closed: falcon-ocr-workers | django-offload
host = "macos-arm64"             # a [[target]] id
python = "3.12"                  # a pinned CPython source release (CPYTHON_RELEASES)
backend = "wasm"                 # closed: native | wasm (verified-subset backend names)
runtime_profile = "wasm-release" # with guest "release", a release_acceptance_matrix lane
execution = "workerd"            # closed: host | workerd
```

The generator `tools/gen_release_matrix.py` validates each row and derives the rest:

- `id` = `tier1-{host}-py{tag}-{backend}`; ids are unique.
- `artifact_target` = the host target's `rust_target` for `native`; `wasm32-wasip1` for `wasm`.
- `(backend, "release", runtime_profile)` must be a `lane` of `config/release_acceptance_matrix.toml`,
  and the backend must not be an `excluded_backend`.
- `python` must be a key of `CPYTHON_RELEASES`. It need not be an admitted target: the matrix records the
  claim, and the gate fails a Tier 1 cell whose Python Molt refuses (3.15 today, V1-35).
- `execution = "workerd"` requires `backend = "wasm"`; `host` requires a native backend.

Output: `src/molt/release_matrix.py` (generated, `--check`) gains `Tier1Witness`, `Tier1Execution`
(`Literal` types), a frozen `Tier1Cell` dataclass and `TIER1_CELLS`. No generated Markdown copy: the
claim page links the TOML.

`config/public_contract_v1.toml` declares `molt.release-targets.v2`, and the contract surface snapshots
`TIER1_CELLS`. A Tier 1 change is therefore a visible `public_contract_gate --update`. That makes the
change reviewable; it does not prove the owner approved it. Owner approval stays a review duty.

`witness` and `execution` are claim data: the contract surface and the claim page consume them. The
release gate proves a Tier 1 cell through its verified-subset coordinate
`{host}-py{tag}-cpython-language-gil-{backend}-release` (`src/molt/verified_subset.py:275-278`), which
runs under node-wasi for WASM, not workerd. The witness-level proof (workerd, Molt-compiled Django
entrypoints) is held by the v0.1-blocking rows that own it (HF-788, HF-790, HF-548) until their
producers exist. Once they do, those runners read `TIER1_CELLS`.

### 4.2 Version rule: one function, three consumers

`tools/release/release_model.py` owns version parsing. It gains
`release_line(version) -> ReleaseLine` with `ReleaseLine = Literal["pre-1.0", "stable"]` (major 0 →
`pre-1.0`). Consumers:

- **Release gate** (`tools/release_exit_gate.py`).
  - `stable`: holding findings = every open row; required roles = E1, E2, E4 and every verified-subset
    coordinate. Unchanged.
  - `pre-1.0`: holding findings = open rows whose v0.1 class is `v0_1_blocking` or absent (absent is
    blocking: an unclassified row cannot leave the release scope by omission, the rule `finding_status`
    already applies to sections). Required roles = E4 and the E3 coordinates of the Tier 1 cells. E1 (the
    numpy/scipy Pact witness) and E2 (the CPython-floor scoreboard, which includes LLVM `release-fast`)
    evidence Tier 2 cells, so they do not hold a 0.x release; a bundle may still carry them. A Tier 1
    cell with no coordinate (its Python is not admitted) is a FAIL input like a holding finding, not an
    assembly problem, so the bundle still assembles and reports.
  - Every open row stays in the bundle's `open_findings` record; that is the report.
- **Release evidence and authority** (`tools/release/release_evidence.py`, `release_authority.py`,
  `release_model.py`, `.github/workflows/release.yml`): H0, full v1 acceptance
  (`verify_full_release_acceptance`) and the all-coordinate E3 provenance apply to `stable`. `pre-1.0`
  verifies the gate report and the Tier 1 E3 provenance. The release manifest's `phase_exit` becomes
  absent for `pre-1.0`, so the manifest schema moves from v5 to v6 and the contract declaration follows.
  `RELEASE_NOTES.md` of a `pre-1.0` release appends the generated known-issues list (4.4).
- **Release candidates**: `_stage_candidate_assets` requires all six targets
  (`release_authority.py:1608-1614`), and `validate_consumer_proof` (`:1422`) requires installed-consumer
  runs for every acceptance lane, LLVM included, times every admitted Python (`:1075-1089`). For
  `pre-1.0` both narrow to the Tier 1 hosts and lanes (section 11).
- **Docs**: only `PUBLIC_CONTRACT_V1.md` "Versioning and release gating" states the rule. The claim page,
  `packaging/PACKAGING.md:20-31`, the gate docstring and the ledger link to it. Section 10 supplies the
  integrator's ledger text.

The bundle schema (`molt-release-exit/4`) does not change: it already records `version` and
`open_findings`; the verifier recomputes holding from the ledger at the source revision.

### 4.3 v0.1 classification: a typed ledger table

`tools/finding_status.py` parses a second membership table in the "Family dependency order"
subsection, header `| Row | v0.1 class |`, into `V01Class` (`StrEnum`: `v0_1_blocking`, `tier2`,
`post_v0_1`). A second table, not a column of the v1 table, so main's ledger stays valid until the
integrator applies the data, and no header-keyed dual schema exists. `project()` reports a duplicate
class and an unknown value in `status.problems`, which the gate already refuses; `dependency_problems`
reports a class on a closed or unknown row. A missing class is not a problem; it holds a `pre-1.0`
release (4.2) and appears as "unclassified" in the known issues.

`tools/ledger_edit.py` gains `v01 set KEY CLASS` (add or restate one row) and `v01 apply FILE` (one write
of every row in a JSON data file); `open` accepts `--v01-class`; `close` and `merge` remove the row's v0.1
class with its v1 class, and a merge keeps the stricter class (`v0_1_blocking` > `tier2` > `post_v0_1`).
The lane delivers the classification as that data file; the integrator applies it. The rule for the data (brief step 4): a row
is `v0_1_blocking` if it fails in a Tier 1 cell, is a safety defect, blocks Tier 1 installed delivery, or
guards a public performance claim; `tier2` if it fails only in Tier 2 cells; otherwise `post_v0_1`.

### 4.4 Known issues: generated, never hand-written

`tools/known_issues.py` renders Markdown from one ledger text and `TIER1_CELLS`: the blocking rows
(including unclassified rows), the Tier 2 rows and the post-v0.1 rows, each with its id and first
sentence. It is pure; callers bind the bytes (the release authority reads the ledger blob at the release
source revision, as the gate does). No tracked known-issues file exists, so ledger edits cause no
generated-file churn.

### 4.5 CI cell selection

No synthesized proof-plan cells: no command can run a Tier 1 cell yet, and the plan refuses a cell
without a command (`tools/proof_plan.py:3247-3248`). The HF-788 (Django) and HF-790 (workerd) runners read
`TIER1_CELLS` when their producers exist; they run in a scheduled tier, not on every pull request.

The CI changes in this lane:

- D4: the Falcon driver command builds release with `wasm-release` and Binaryen `Oz`, but sits in the
  `dev-fast` cell whose other commands build dev. It moves to a new cell
  `linux-x86_64-py312-wasm-release` (profile `wasm-release`).
- Experimental backends leave every presubmit and postsubmit tier. Deleted: `luau.compile.*`,
  `rust.clippy.backend-luau-only`, `rust.clippy.backend-rust-only`, the `backend-luau` and
  `backend-rust` landing rules, the `lune` toolchain and its CI provisioning, and the Luau and
  Rust-emitter packages, features and tests inside `rust.test.compiler-authorities`, the backend
  daemon gates and the `runtime.function-module-contract` lane. `mlir.test.backend` keeps its cell
  but runs on the schedule only: it is the plan's one host-C++ build, which the toolchain-capture
  tests use. Proof-plan validation refuses a command outside the schedule, a landing rule or a named
  lane whose argv selects an excluded backend (`--target luau|rust|mlir`, a Cargo `-p` of
  `molt-backend-luau`, `molt-backend-rust` or `molt-backend-mlir`, their backend features, or a `cwd`
  in those crates). The exclusions are read from `config/release_acceptance_matrix.toml`. The
  proof-plan backend value `rust` (the Cargo workspace) is a different axis and stays.

### 4.6 Public claim

`docs/spec/PUBLIC_CLAIM_V0_1.md`: one hand-written page. It names the two witnesses, links `config/release_targets.toml`
and the known-issues command, and links the version rule. `README.md` and
`docs/getting-started.md` link it instead of restating support facts.

## 5. Invariants

1. Tier 1 is written in exactly one file. Every other surface is generated from it or links to it.
2. A Tier 1 change changes the public contract surface.
3. `stable` releases: unchanged hold (every open row; full E3; H0).
4. `pre-1.0` releases: held by `v0_1_blocking` and unclassified rows and by Tier 1 E3 coordinates; never by
   Tier 2 rows or cells. Invariants 3, 4 and 6 are the release-line slice's (section 11); until it lands,
   every version keeps the 1.x hold.
5. No presubmit or postsubmit cell runs an experimental backend; the schedule may.
6. A Tier 1 Python that Molt does not admit fails the gate; it is never silently skipped.

## 6. Costs

All changes are build-time or release-time. The generator reads two more small TOML files. The gate
reads the ledger blob it already reads. The proof plan gains one cell and loses the experimental commands. No
emitted program changes; no runtime cost.

## 7. Consumers changed

`tools/gen_release_matrix.py`, `src/molt/release_matrix.py` (generated), `config/release_targets.toml`,
`config/public_contract_v1.toml`, `config/public_contract_v1.surface.json` (generated),
`tools/public_contract_gate.py`, `tools/generator_manifest.toml` (sources), `tools/finding_status.py`,
`tools/ledger_edit.py`, `tools/known_issues.py` (new), `tools/release_exit_gate.py`,
`tools/release/release_model.py`, `tools/release/release_evidence.py`,
`tools/release/release_authority.py`, `.github/workflows/release.yml`, `tools/proof_plan.py`,
`tools/proof_plan.toml`, `.github/proof-plan.generated.json` and `docs/agent/PROOF_PLAN.generated.md`
(generated), `docs/spec/PUBLIC_CONTRACT_V1.md`, `docs/spec/PUBLIC_CLAIM_V0_1.md` (new),
`packaging/PACKAGING.md`, `README.md`, `docs/getting-started.md`, and their direct tests. Every edit of
`config/release_targets.toml` re-keys E3 receipts (`tools/verified_subset.py:118`); that is correct for
a claim change.

## 8. Test plan

- Generator: a Tier 1 row with an unknown host, an unpinned Python, an excluded backend, a lane absent
  from the acceptance matrix, or `workerd` on native is refused; ids and artifact targets derive as
  stated; `--check` detects drift.
- `finding_status`: the v0.1 table parses; a class on a closed row, a duplicate and an unknown value are
  problems; a missing class is not.
- `ledger_edit v01 set` and `v01 apply`: add, restate, bulk apply; refusals for closed rows and unknown
  classes; `open --v01-class`; `close` and `merge` clear the class; a merge keeps the stricter class, and
  an unclassified side counts as `v0_1_blocking`.
- Release gate (tests inject `TIER1_CELLS`, because the real 3.15 cells always FAIL today): for 0.x, a
  ledger with only `tier2` open rows does not hold; an unclassified or `v0_1_blocking` row holds; a row
  classed twice is a problem; a Tier 1 cell whose Python is not admitted FAILs but the bundle assembles;
  a 0.x bundle without E1/E2 and with only Tier 1 E3 roles passes; for 1.x every open row holds and E3,
  E1 and E2 are required. Boundary versions `0.99.99` and `1.0.0`. Existing tests that pin "every
  release is held" are rewritten to the new rule, not deleted.
- Release evidence and authority: a 0.x index without H0 succeeds; a 1.x index without H0 is refused;
  release notes come from the ledger at the source revision.
- Known issues: rendering from a fixture ledger groups and orders rows as stated.
- Proof plan: a command whose argv selects an excluded backend is refused; the Falcon driver command
  runs in `linux-x86_64-py312-wasm-release`.
- Owning suites: `tests/tools/test_release_supply_chain.py`, `tests/tools/test_public_contract_gate.py`,
  the release-gate tests, `tests/test_proof_plan.py`, the ledger tool tests.

## 9. Review record

Round 1 (2026-11-05): `egm-design` and `egm-implementation-readiness`, each given only this file.

- BLOCKER (both): `tools/release/release_evidence.py` requires H0, full v1 acceptance and all 108 E3
  roles for every version, so a 0.x release could never pass. **Fixed**: 4.2 moves release evidence,
  authority, model and `release.yml` with the gate.
- BLOCKER (design): E1 and E2 hold 0.x releases on Tier 2 cells. **Fixed**: they are required for
  `stable` only (4.2); the parent reviews this reading of the owner rule.
- BLOCKER (design): `runtime_profile`, `execution` and `witness` had no gate consumer. **Answered**: they
  are claim data; the gate proves the E3 coordinate; witness-level proof is held by the owning rows
  (4.1).
- BLOCKER (design): duplicate homes remain (HF-620 row text, ledger lines 782/1379/1383,
  `packaging/PACKAGING.md:20-31`, gate docstring); the design added prose copies. **Fixed**: one prose
  home (`PUBLIC_CONTRACT_V1.md`), the others link; section 10 supplies the HF-620 row text too.
- BLOCKER (readiness): an unadmitted Tier 1 Python as an assembly problem would stop the bundle.
  **Fixed**: a FAIL input (4.2).
- BLOCKER (readiness): synthesized cells have no command and the plan refuses them. **Fixed**: no
  synthesis (4.5).
- BLOCKER (readiness): an excluded-backend rule on cells hits the `rust` (Cargo) cells and misses Luau
  commands in other cells. **Fixed**: the rule reads command argv; the named commands are listed (4.5).
- BLOCKER (readiness): duplicate classes resolve last-wins, and `close`/`merge` leave v0.1 classes.
  **Fixed**: duplicates are `project()` problems; `close`/`merge` clean up; merge keeps the stricter class.
- NOTE: a column instead of a second table. **Answered**: a second table keeps main's ledger valid until
  the integrator applies the data, without a dual-schema parser (4.3).
- NOTE: the `wasm-dev` relabel would misdescribe the cell's dev commands. **Fixed**: the one release
  command moves to its own cell (4.5).
- NOTE: cut the generated Markdown; known issues could live in `finding_status`. **Fixed** (the first);
  **answered** (the second): `tools/known_issues.py` is the renderer and CLI, and reads the ledger only
  through `finding_status`.
- NOTE: release candidates for all six targets, generator import list, surface schema, PROOF_PLAN
  generated doc, `aarch64`/`arm64` spelling. **Fixed** in 4.2, section 7 and the implementation; the
  candidate set is a decision for the parent (4.2).

Round 2 (2026-11-05): the same reviewers, given only the revised file.

- BLOCKER (design): release candidates and the consumer scope still require Tier 2 hosts, lanes and
  Pythons. BLOCKER (design): E1's registry stays a required root key. BLOCKER (both): "a bundle may
  still carry E1/E2" leaves gate behavior undecided. **Cut**: the release-line migration is section 11,
  with the exact `pre-1.0` role set decided; this lane does not change release code.
- BLOCKER (design): HF-788 and HF-790 are not on main. **Answered**: the integrator opened them in the
  ledger lane; section 10 supplies their exit duty.
- NOTE: the argv rule misses `--workspace` clippy and `cwd`-selected crates. **Answered**: invariant 5 is
  "no command runs or compiles an experimental backend's tests or programs"; whole-workspace lint with
  the backends' features off compiles their code but runs nothing of theirs, and stays.
- NOTE: tests pin `mlir.test.backend` and `lune`. **Fixed** in the implementation with the commands.
- NOTE: test plan out of date; merge order for unclassified; ledger line numbers; HF-626; version
  guards. **Fixed** (sections 8, 10, 11).

## 10. Supplied text for the integrator's ledger edit

1. In "Release decision and exit scope", replace the text from "`tools/release_exit_gate.py` reads this
   file" to the end of that paragraph with:

   > `tools/release_exit_gate.py` reads this file at the release source commit; the version rule is
   > stated once in [the public contract](../spec/PUBLIC_CONTRACT_V1.md#versioning-and-release-gating)
   > (owner decision 2026-11-05: the first release is v0.1). Each open row carries one v0.1 class in the
   > family dependency order; an unclassified row holds a 0.x release.

2. HF-788 and HF-790 (opened by the integrator in the ledger lane, not yet on main) hold the
   witness-level proof. Their exits add the duty: "once the producer exists, the release gate requires
   its receipt for each Tier 1 cell of this witness". Class: `v0_1_blocking`.
3. Restate the HF-620 row's finding cell: "The v0.1 support matrix is decided (owner, 2026-11-05): Tier 1
   is the four witness cells in `config/release_targets.toml`; every other cell is Tier 2." Its exit:
   "The release gate, release evidence and the claim read `TIER1_CELLS` (section 11); every open row has
   a v0.1 class." The rows that restate the 10-30 Tier 1 link to the same file: HF-613, HF-621, HF-626
   ("Windows … a Tier 1 target"; Windows is Tier 2) and the family T description.

## 11. Release-line slice (cut from the HF-620 lane after round 2)

Two review rounds showed that "a 0.x release is held only by Tier 1" changes the release pipeline in
more places than the gate. The rule says to cut scope after two rounds, so this lane does not change
release code; the gate and release tooling still hold every version to the 1.x bar. The next slice
moves these consumers in one landing, with the rewritten tests:

| Consumer | Today | `pre-1.0` |
| --- | --- | --- |
| `tools/release_exit_gate.py` `_derive_status`, `source_findings` (`:141-144`, `:575-588`) | every open row holds | `v0_1_blocking` and unclassified rows hold |
| gate E3 roles (`:106-117`, `:780-794`, `:913`, `:1141-1149`, `:1223`) | all 108 coordinates | `tier1_coordinates()`; a cell without a coordinate is a FAIL input |
| gate E1 registry and receipts (`:78-88`, `:352-359`, `:758-770`, `:836-839`, `:1137-1138`) | required root key and two receipts | absent; the exact `pre-1.0` role set refuses E1 and E2 |
| gate E2 scoreboard (`:848-855`, `:1186-1194`) | required | absent |
| `tools/release/release_evidence.py` (`:107-118`, `:155-204`) | H0, full v1 acceptance, all-coordinate E3 provenance; version from the caller | Tier 1 E3 provenance; version from the gate report |
| `tools/release/release_authority.py` `resolve_source` (`:316-317`), candidates (`:1608-1614`), consumer scope (`:1075-1089`), notes (`:1709-1716`) | H0 planned; six targets; every lane × Python | no H0; Tier 1 hosts and lanes; notes append `tools/known_issues.py` |
| `tools/release/release_model.py` (`:251`, `:280-296`, `:326`) | `phase_exit` required | absent; manifest schema v6 |
| `.github/workflows/release.yml` (`:304-392`) | always signs H0 | `stable` only |

An unreadable version selects the `stable` rule. The decided `pre-1.0` role set is exact: E4 plus the
Tier 1 E3 roles; carried E1/E2 or non-Tier-1 E3 roles are refused, so a stale Tier 2 artifact cannot
fail a 0.x release by accident.
