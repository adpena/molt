<!-- Foundation blueprint 59. Arc: THE SEMANTIC FACT PLANE — the meta-architecture
that underpins both correctness and performance ("one authority per invariant";
retire classes of wrongness/slowness via first-class GENERATED, checkable facts;
rustc-gated exhaustive matches make drift impossible). Author: portfolio-architect.
Date: 2026-06-23. Status: EXECUTABLE PLAN; Phase 0 metric correction implemented 2026-06-26.
Original session was design-only; Phase 0 is now integrated, and later phases still require current code verification. Every load-bearing claim is anchored
to a file:line verified against the worktree at authoring time; code beats this doc —
re-verify before acting. Composes with: 25 (op-kind registry), 46 (semantic control
plane), 47 (CallFacts), 51 (10-year roadmap — names this "SEMANTIC FACT PLANE"), 52
(autonomous charter C.3 #13 — "fact-plane completeness as a standing institution"),
21/21a-e (decomposition program). Number 59 chosen: docs 50-52 exist, 53-58 free; this
is the first 53-69 doc, taking 59 per the assignment (50-69 band, 59 requested + free). -->

# 59 — The Semantic Fact Plane: one generated authority per invariant

> **Status: EXECUTABLE PLAN; Phase 0 + Phase 4 fact-migration LANDED (verified 2026-06-27);
> Phases 1-3 (generator manifest + meta-gate) still to build.** This is the
> meta-architecture doc 51 §1 calls the cure ("a SEMANTIC FACT PLANE") and doc 52 §C.3 #13
> calls the 50-year institution ("every IR fact has a generated producer/consumer/transport
> contract with drift detection — the op_kinds model extended to all fact families"). It does
> not introduce a *new* fact family; it makes the *machinery* for fact families a
> first-class, generated, CI-gated, idempotent institution, and it fixes the one
> structural defect in that machinery discovered in execution: the
> `structural_audit` Phase 0 now uses kitchen-sink and undecomposed metrics so correct
> decomposition is credited.
>
> **What is DONE (re-verified against the worktree, 2026-06-27):** Phase 0 (the metric
> correction, §6) and the Phase 4 fact-migration ladder (§8) — every hand-classified opcode
> fact is now in `op_kinds.toml` and read through a generated predicate. The §0 ratchet rows
> `hand_classified_matches`, `handset_classifications`, and `critical_hand_classifications`
> are all at the end-state target **0** (`tools/structural_audit.py --check` GREEN;
> `gen_op_kinds.py --check` in sync; `cargo check -p molt-ir` clean). **What REMAINS:**
> Phases 1-3 — `tools/generator_manifest.toml`, `tools/check_generator_manifest.py`, and the
> reusable closed-domain exhaustiveness auditor do **not** exist yet; the generator-gating
> holes catalogued in §4 are still open (only `gen_op_kinds.py` is CI `--check` gated). Those
> phases are the next structural unit for this arc.

---

## 0. The end-state outcome (stated crisply)

**Five-to-fifty-year end state.** Every semantic invariant in molt — every property
a pass or a backend must *agree on* to be correct or fast — has **exactly one
generated authority**, rendered from a declarative table by a `gen_*.py` generator
that is `--check` gated in CI and idempotent, consumed by every site through the
generated predicate, and made drift-proof by a rustc-gated *exhaustive* match (a new
enum variant fails to **compile** until classified). The hand-maintained-fact surface
is zero and stays zero because adding a new fact follows ONE documented workflow and
adding a new opcode/terminator/fact-family member is a **build error** until the table
row exists.

**The machine-checkable form of "done"** (the `structural_audit` ratchets, doc 46 §2).
The `now` column is the live value in `tools/structural_audit_baseline.json`, re-verified
against the worktree on 2026-06-27 (`tools/structural_audit.py --check` GREEN). The
fact-migration rows — `hand_classified_matches`, `handset_classifications`,
`critical_hand_classifications` — have all **reached the end-state target 0**: the Phase 4
ladder (§8) landed every opcode-fact classifier into `op_kinds.toml` (git trail:
`2891edfc4` raw-i64 lowering facts, `98e189a30` refcount balance roles, `659cc2732` TIR
state-machine facts, `b9c19fa42` generator fusion poll roles, `7b92eeb6f` residual TIR
semantic roles, and siblings). The zero is "clean code," not "blind gate": the probe is
proven to still *find* debt — a synthetic silent-default `match` over 4 opcodes and a
synthetic ≥3-opcode `matches!` set both trip `probe_semantic_fallthroughs`.

| ratchet | meaning | was | 2026-06-23 | now (2026-06-27) | end-state target |
| --- | --- | --- | --- | --- | --- |
| `duplicate_authorities` | one property classified in ≥2 files | — | 0 | **0** | **0** (LOCKED) |
| `hand_classified_matches` | `match {… _ => value}` opcode classifier w/ silent default | 57 | 44 | **0** | **0** (REACHED) |
| `handset_classifications` | `matches!(x, OpCode::A \| B \| …)` ≥3-opcode implicit-false set | 48 | 29 | **0** | **0** (REACHED) |
| `critical_hand_classifications` | the above in an RC/alias/escape/codegen file | 6 | 1 | **0** | **0** (REACHED) |
| `debt_markers_total` | TODO/FIXME/HACK/WORKAROUND/"for now" | — | 526 | **343** | monotone down |
| `kitchen_sink_files` | files over the decomposition ceiling with concern-mixing top-level regions | — | (RED, raw `god_files` 57) | **0** | **0**; driven down by extracting mixed authorities, never inflated by cohesive decomposition products |
| `undecomposed_god_files` | files over the decomposition ceiling with no sibling decomposition package/stem directory | — | (RED) | **0** | **0**; a lone monolith remains red until the decomposition package exists |
| `max_undecomposed_file_lines` | the largest over-ceiling file with no decomposition context | — | (RED) | **0** | monotone down; decomposition residuals remain board-visible but are excluded from this regression metric |
| `large_source_file` findings | raw over-ceiling source size | — | — | board-only | board-only triage; not a ratchet metric |

The fact-migration rows are at their terminal value **0**; the institution's remaining job
for them is to *keep* it 0 — every new opcode-fact classifier is caught by
`structural_audit.py --check` and must go down the §5 add-a-fact workflow. The
decomposition rows went green via the Phase 0 metric correction (§6) plus the 21a-e
crate/package extractions.

When every row is green AND the green is *load-bearing* (the gate consumes generated
facts, not heuristics — doc 46 rule #1), molt has a **semantic nervous system**
(doc 51 §7): no pass-local reasoning, no reconstruct-from-low-level-events, every
Python-visible property a first-class cached fact with a checkable obligation.

**The class this arc retires:** *duplicate-authority drift* and *missed-fact-on-new-
member* — the single most prolific silent-miscompile family in molt's history (doc 25
§1: five proven instances, escalating to UAF under drop insertion). Not one instance;
the **ability to express** "two tables that can disagree" and "a new opcode the
oracle silently defaults" is removed from the codebase.

---

## 1. Time-traveler derivation: from the 50-year outcome back to the facts to build

Working **backward** from "every invariant has one generated, checkable authority":

1. **For the end state to hold, "two authorities for one invariant" must be
   UNEXPRESSIBLE.** → The authority must be *generated* from a single table, and the
   generator must be `--check` gated so a hand-written second copy is caught. (Built
   for op-kinds: `tools/gen_op_kinds.py` + `tests/test_gen_op_kinds.py`; the model to
   generalize.)
2. **For "a new member silently mis-classified" to be UNEXPRESSIBLE,** the generated
   authority over a *closed* domain (an enum) must be an **exhaustive `match` with no
   wildcard** so rustc refuses to compile an unclassified variant. (Built: the
   effect oracle renders exhaustively over `OpCode`; `gen_op_kinds.py` validates the
   `[[terminator]]` section is exhaustive over the `Terminator` enum,
   `tools/gen_op_kinds.py:888`.) Over an *open* domain (wire-kind strings), the
   authority is a total table + a `--check` drift audit against the producer
   (`tools/audit_op_kinds.py`).
3. **For the authority to be TRUSTED,** discovery (ranking candidates) must be
   separable from authority (gating behavior): heuristic regex may *find* a
   candidate, but only a generated fact or typed AST may *gate* (doc 46 rule #1, the
   `_count_enum_variants` parser-bug lesson). → The fact plane needs a **two-tier
   contract** baked into every generator: a discovery ranker (advisory) and an
   authoritative gate (consumes the generated artifact).
4. **For the institution to SURVIVE adding the next 20 fact families,** there must be
   ONE registry of "what is a generated authority, what gates it, is it idempotent"
   — a *generator manifest* — and ONE meta-gate that fails if a generated file has no
   committed generator, a generator has no `--check`, or a generator is
   non-idempotent. (Today: 8 `gen_*.py` scripts exist; only 4 are `--check` gated in
   CI; this is the gap — §4.)
5. **For decomposition (a correctness/clarity good) not to FIGHT the institution,**
   the ratchet that measures "kitchen-sink debt" must credit cohesive decomposition
   products instead of counting every-file-over-a-line-ceiling — otherwise splitting
   a god-file *raises* the debt number and the ratchet can never green. (This is the
   live defect: §6.)
6. **For the fact plane to PAY for itself in perf,** each fact must be *attached to
   the IR and survive every representation boundary* to the backend that consumes it,
   measured by a *positive* ratchet (attached-fact coverage only goes up). (Built:
   `tools/call_fact_coverage.py`; the dual of the debt ratchet — doc 46 §2.)

Items 1-3 are **built for op-kinds** and must be **generalized into a stated
institution** (this doc). Item 4 is the **missing meta-gate** (§4). Item 5 is the
**live structural defect** (§6). Item 6 is the **bridge to the perf ladder** (§7).

---

## 2. What exists today (verified anchors — do not duplicate or contradict)

The fact plane is **already real for two fact families**. This arc *advances and
composes*; it does not restart.

### 2.1 The op-kind registry (the proven thesis — doc 25)
- **Table:** `runtime/molt-ir/src/tir/op_kinds.toml` (3146 lines, verified). The
  single source of truth for the cross-component op-"kind"-string vocabulary AND a
  growing set of per-OpCode facts: `may_throw`/`side_effecting`/`purity`,
  `operand_ownership`, `result_absorbs_operands`,
  `result_mints_owned_selected_operand`, plus ~30 classifier/fact sets enumerated in
  the header (`op_kinds.toml:14-91`) — fresh-value/owned-alias/inert/transparent-
  alias classifiers, alias MemRegion roles, `fusion_barrier_opcodes`,
  `i64_zero_divisor_guard_opcodes`, `i64_shift_count_guard_opcodes`,
  canonicalize facts, `[[terminator]]` ownership, the three frontend `op.kind`
  tables (`frontend_raising_kind` / `frontend_check_exception_skip` /
  `binary_op`).
- **Generator:** `tools/gen_op_kinds.py` (2761 lines, verified). Renders
  `runtime/molt-ir/src/tir/op_kinds_generated.rs` (3906 lines) +
  `src/molt/frontend/lowering/op_kinds_generated.py` (490 lines). Fail-loud
  validation (`load_table`, `tools/gen_op_kinds.py:198`) rejects a malformed/typo'd
  table rather than silently degrading. The effect oracle is rendered as an
  EXHAUSTIVE `match` over `OpCode` (no wildcard) — the structural kill for the
  `matches!`-default-false trap (`gen_op_kinds.py:17-20` docstring; the rendered
  function in `op_kinds_generated.rs`).
- **Sync gate:** `tests/test_gen_op_kinds.py` re-renders in memory and asserts byte
  equality (the `tests/test_gen_intrinsics.py` pattern). CI:
  `tools/gen_op_kinds.py --check` (`.github/workflows/ci.yml:56`).
- **Producer-drift gate:** `tools/audit_op_kinds.py --check` (`ci.yml:59`) — the
  *open-domain* complement: catches a frontend-emitted wire kind with no table row.
- **The cross-axis invariant kill is the template for all future facts:**
  `gen_op_kinds.py:238-247` rejects a `purity = "pure"` row with `may_throw = true`
  (a pure op is nothrow) and a `purity = "pure_may_throw"` row with
  `may_throw = false`. This is the exact bug that let DCE drop a dead `1 << -1`
  (`gen_op_kinds.py:233-237`). **Lesson to institutionalize: when two columns are
  two views of one property, the generator asserts their agreement — drift inside a
  single table is also drift.**

### 2.2 The structural-audit instrument (doc 46 §2, the debt ratchet)
- **Tool:** `tools/structural_audit.py` (910 lines, verified). Probes:
  `probe_semantic_fallthroughs` (hand-classified `match`/`matches!` over opcodes,
  EXCLUDING fail-loud and emitter defaults — `:199-217`), `probe_god_files`
  (`:371`), `probe_debt_markers` (`:416`), `probe_duplicate_authorities` (`:462` —
  delegating consumers correctly excluded, `:511`), `probe_registry_reconciliation`
  (`:573` — INFO only; the effect oracle is rustc-gated so coverage is compiler-
  enforced).
- **Ratchet:** `ratchet_metrics` (`:629`) + `--check` (`:869`) +
  `tests/test_structural_audit.py`. Baseline `tools/structural_audit_baseline.json`.
  Board `docs/design/foundation/STRUCTURAL_AUDIT_BOARD.md` (regenerated by
  `--write-board`).
- **The binding rules it already encodes** (do not weaken): discovery-vs-authority
  (`:730-733`), deletion candidates carry a replacement authority + equivalence gate
  (`_DELETION_PLAYBOOK`, `:664`), the tool names its own limitations (`_TOOLING_GAPS`,
  `:678`).

### 2.3 The positive ratchet (doc 46 §2, the coverage dual)
- `tools/call_fact_coverage.py` (verified) tracks per-call-fact whether it is
  ATTACHED / OPCODE_STATIC / TRANSIENT; `--check` fails if attached-fact count
  DECREASES. CI: `ci.yml:68`. This is the *perf-facing* half of the plane (§7).

### 2.4 The other generators (the institution is INCOMPLETE — §4)
Verified present: `gen_op_kinds.py`, `gen_protocol.py`, `gen_intrinsics.py`,
`gen_compat_platform_availability.py`, `gen_diff_lanes.py`,
`gen_luau_support_matrix.py`, `gen_stdlib_module_union.py`,
`gen_stringprep_tables.py`. **CI `--check` gated:** only `gen_op_kinds.py`
(`ci.yml:56`). Pytest-covered: `gen_op_kinds`, `gen_intrinsics`, `gen_stringprep`.
**`gen_protocol.py` (the F1 mixin-decomposition shim, `tools/gen_protocol.py:1`) has
NO direct `--check` in CI** — it is pinned only indirectly by
`tests/test_frontend_package_composition.py`. The remaining four generators have
neither a CI `--check` nor a sync test. **This is a fact-plane completeness hole:
a generated file with no committed, gated generator is structural debt by the
project's own definition (`gen_protocol.py:25`).**

### 2.5 The decomposition program (21/21a-e) — composes, with a metric collision
- `function_compiler.rs` is being split into a cohesive `fc/` family: **40
  submodules** verified present
  (`runtime/molt-backend/src/native_backend/function_compiler/fc/`: `arith.rs`,
  `control_flow.rs`, `dict_ops.rs`, `exceptions.rs`, `generators.rs`, … `mod.rs`) —
  exactly the Lattner "one responsibility per file" ideal — **alongside** a residual
  `function_compiler.rs` still at ~28K lines (board row 2). The split is in flight.
- `src/molt/cli/` is now a package (`__init__.py` + `arg_helpers.py`, `completion.py`,
  `deps.py`, `maintenance.py`, `native_toolchain.py`, `wasm.py`, …) — but the bulk
  still lives in `cli/__init__.py` (~41.6K lines; the board's "`cli.py`" row is
  actually `cli/__init__.py`). `frontend/visitors/` (`calls.py`, `classes.py`,
  `comprehensions.py`, `pattern_match.py`) and `frontend/lowering/` are cohesive
  products.
- **The collision (measured at authoring time, 2026-06-23 — NOW RESOLVED):** at
  authoring time `structural_audit.py --check` was **RED** — the raw `god_files: 53 → 57`
  and `max_god_file_lines: 39520 → 41266` count metrics both *regressed* because of correct
  decomposition, while `hand_classified_matches` (57→44), `handset_classifications` (48→29),
  and `critical_hand_classifications` (6→1) all *improved*. Decomposition was making the
  codebase structurally better and the ratchet redder. **§6 was the structural fix and it
  has landed (Phase 0, §8): the gate now ratchets `kitchen_sink_files` /
  `undecomposed_god_files` / `max_undecomposed_file_lines` instead of raw line count, and as
  of 2026-06-27 every one of those is `0` and `structural_audit.py --check` is GREEN.** The
  raw-size signal survives as board-only triage (`large_source_file`), not a ratchet.

---

## 3. The structural facts / mechanisms this arc builds (each tied to the class it retires)

This arc's deliverable is **not "faster code" or "fewer bugs"** — it is **new
machinery that makes a class of structural failures unexpressible**. Six mechanisms,
each tied to a class.

### F1. The Generator Manifest + meta-gate — retires *"a generated file can drift because its generator isn't gated / isn't idempotent / doesn't exist"*
The single declarative registry of every generated authority and its contract.
- **Artifact:** `tools/generator_manifest.toml`. One `[[generator]]` row per
  `gen_*.py`: `tool`, `outputs[]` (the generated files), `source` (the table/AST it
  reads), `check_mode` (`true` once it has `--check`), `idempotent` (`true` once
  proven), `sync_test` (the pytest pinning it, or a `reason` it needs none — e.g.
  `gen_protocol.py` is import-only, byte-identity is not the gate), `closed_domains[]`
  (the enums it renders exhaustively, for the rustc-gate audit), `discovery_only`
  (`false` for authorities; `true` for ranking tools like `structural_audit.py`).
- **Meta-gate:** `tools/check_generator_manifest.py --check` (+
  `tests/test_generator_manifest.py`). It FAILS if: (a) a `@generated` file under
  `runtime/`/`src/`/`tools/` has no manifest row (the "generated file with no
  committed generator" debt, made machine-checkable — generalizing
  `gen_protocol.py:25`); (b) a manifest row has `check_mode = false`; (c) a row's
  generator is non-idempotent (the gate runs the generator twice into a temp dir and
  diffs — `gen` then `gen` again must be byte-identical, the
  `tests/test_gen_op_kinds.py` discipline lifted to the manifest level); (d) a
  declared `closed_domain` enum gained a variant not covered by the exhaustive render
  (cross-checks the §F2 audit).
- **Why a table, not prose:** the manifest IS a fact family — "what are the
  authorities" is itself an invariant with one authority. It is consumed by CI, by
  `structural_audit.py` (to know which files are generated and skip them — replacing
  the heuristic `_is_generated`, `structural_audit.py:87`, with the authoritative
  list), and by future fact-family authors as the canonical "add your generator
  here" checklist.
- **Class retired:** *ungated/undiscoverable/non-idempotent generated authority.*

### F2. The Closed-Domain Exhaustiveness Auditor — retires *"a new enum variant silently defaults in some hand-written classifier"*
Generalizes the `[[terminator]]`-exhaustiveness check (`gen_op_kinds.py:888`) and the
`binary_op` ast.operator-exhaustiveness check (`gen_op_kinds.py:1043`) into a reusable
mechanism keyed off the manifest's `closed_domains[]`.
- **Mechanism:** for each closed domain (e.g. `OpCode` in `ops.rs`, `Terminator` in
  `blocks.rs`, `ast.operator`), the generator that owns it renders an exhaustive
  match; the manifest meta-gate verifies (i) the render has no wildcard arm, and
  (ii) the table is total over the live enum (parse the enum with the robust
  `_count_enum_variants`, `structural_audit.py:539`, which is already unit-tested
  against payloads/discriminants, `tests/test_structural_audit.py:140`).
- **The discovery-vs-authority seam, honored:** `_count_enum_variants` is *discovery*
  (it ranks "is the table total?"). The *authority* that a fact is correct stays the
  rustc exhaustive match — if the parser under-counts, the worst case is a false
  "looks total"; the rustc match still refuses an unclassified variant at compile
  time. The parser can never *manufacture* a passing gate that rustc would fail.
- **Class retired:** *missed-fact-on-new-opcode/terminator/operator.*

### F3. The Add-A-Fact Workflow (the documented, single path) — retires *"facts are added ad-hoc, so the next one re-introduces a second authority"*
A first-class, copy-pasteable procedure (this doc §5) for the two domain shapes,
encoded as a checklist the manifest meta-gate can partially enforce. Without ONE
workflow, every new fact family is a fresh chance to put the classifier in a `.rs`
file. The workflow makes "add a column to the table + regenerate + read the generated
predicate" the path of least resistance.
- **Class retired:** *new-authority-by-default* (the entropy that re-creates drift).

### F4. The structural_audit metric redefinition (§6) — retires *"the debt ratchet penalizes correct decomposition, so it can never green"*
Replace the raw `god_files` count-over-threshold with a **concern-mixing** signal
that credits cohesive decomposition products and a **per-file regression** on
`max_god_file_lines` that excludes in-progress decomposition residual. Detail in §6.
- **Class retired:** *Goodhart collision between two structural goods* (decomposition
  vs. anti-kitchen-sink), which today forces a choice between a red ratchet and a
  re-pinned baseline (both forbidden by CLAUDE.md / doc 52 §A.2).

### F5. The Authority Manifest extension (planning-doc routing → fact-plane routing)
`docs/design/foundation/authority_manifest.toml` (verified) routes *planning
authority* docs for `tools/check_docs_architecture.py`. Extend the same idea to
**semantic authorities**: the generator manifest (F1) is the runtime/compiler analog
— "which file is the authority for invariant X". Cross-link them so a reader (human or
agent) has one place that answers "where is the authority for finalizer-ordering?
for call-target? for op effects?" This is doc 52 §C.3 #13's "standing institution"
made navigable.
- **Class retired:** *unfindable authority* (the "which of N files decides this?"
  archaeology that precedes every drift bug).

### F6. The fact-coverage bridge (compose with the perf ladder — §7)
Not new machinery; a stated *contract* that every new fact family registered in F1
also registers its ATTACHED-coverage row in `call_fact_coverage.py` (or a sibling
coverage tool), so the debt ratchet (down) and the coverage ratchet (up) move
together. A fact that is generated but never attached to the IR is a *latent* fact —
correct but perf-inert. F6 makes "generated but un-attached" visible.
- **Class retired:** *generated-but-dead facts* (the gap between "the table knows" and
  "the backend can use it").

---

## 4. The completeness gap, concretely (what F1 must close first)

Audited this session (`.github/workflows/ci.yml`, `tools/gen_*.py`,
`tests/test_gen_*.py`):

| generator | outputs | `--check` in CI | sync test | idempotent (proven) | manifest action |
| --- | --- | --- | --- | --- | --- |
| `gen_op_kinds.py` | `op_kinds_generated.rs`, `op_kinds_generated.py` | **yes** (`ci.yml:56`) | `test_gen_op_kinds.py` | by sync test | reference row |
| `gen_intrinsics.py` | `intrinsics/generated.rs` | no | `test_gen_intrinsics.py` | by sync test | add `--check` to CI |
| `gen_protocol.py` | `_protocol.py`, `_protocol_attrs.py` | no | indirect (`test_frontend_package_composition.py`) | by composition test | add `--check`; declare `sync_test reason` (import-only) |
| `gen_stringprep_tables.py` | stringprep tables | no | `test_gen_stringprep_tables.py` | by sync test | add `--check` to CI |
| `gen_compat_platform_availability.py` | compat availability | no | **none** | **unproven** | add sync test + `--check` |
| `gen_diff_lanes.py` | differential lanes | no | **none** | **unproven** | add sync test + `--check` |
| `gen_luau_support_matrix.py` | luau support matrix | no | **none** | **unproven** | add sync test + `--check` |
| `gen_stdlib_module_union.py` | stdlib module union | no | **none** | **unproven** | add sync test + `--check` |

**Finding:** of 8 generated authorities, **1** is fully gated in CI; **3** have a sync
test but no CI `--check`; **4** have neither. The fact-plane institution exists in
*spirit* (the op-kinds exemplar) but not in *enforcement* across the family. F1 makes
the contract uniform and the meta-gate makes a new ungated generator a build error.

---

## 5. The Add-A-Fact Workflow (F3 — the single documented path)

This is the procedure every future fact family follows. It is the antidote to
"new authority by default."

### 5.1 Domain shape A — a per-member fact over a CLOSED enum (e.g. a new per-OpCode property)
1. **Add the column** to the owning member rows in the table (e.g. a new
   `my_property = …` field on every `[[opcode]]` in `op_kinds.toml`). Because the
   render is exhaustive, you CANNOT add it to some rows only — the generator fails
   loud on a missing field (the `operand_ownership` mandatory-field pattern,
   `gen_op_kinds.py:248-252`).
2. **Add fail-loud validation** in `load_table` for the column's value set (the
   `_PURITY_VALUES`/`_OPERAND_OWNERSHIP_LEAVES` pattern, `gen_op_kinds.py:62-94`).
   If the new column is two views of an existing property, add the cross-axis
   agreement assertion (the purity↔may_throw kill, `gen_op_kinds.py:238`).
3. **Render** an exhaustive `match`/table in `_render_rs_unformatted`
   (`gen_op_kinds.py:1080`). No wildcard for closed domains.
4. **Replace the hand-written classifier** at every site with a call to the generated
   predicate. Delete the local `match`/`matches!`. (`structural_audit.py`'s
   deletion-candidate board, `STRUCTURAL_AUDIT_BOARD.md`, ranks exactly these sites.)
5. **Register** in `generator_manifest.toml`: add `closed_domains = ["OpCode"]` if not
   present; the meta-gate now guards exhaustiveness.
6. **Test at the consumer** (doc 52 §B.3 / step 4 landmine: facts die at
   representation boundaries — serialization, round-trips, re-lifts). Add the
   round-trip regression: the fact survives `lower_to_simple` → re-lift
   (`ssa.rs kind_to_opcode`) → backend.
7. **Coverage row** (F6): if the fact is IR-attached and perf-relevant, add its
   ATTACHED-coverage entry to `call_fact_coverage.py`'s `CALL_FACTS`.

### 5.2 Domain shape B — a fact over an OPEN domain (e.g. a new wire-kind classifier set)
1. **Add the set/row** to the table (a `classifier_*` array or a `[[kind]]`/
   `[[consuming_kind]]`/… row). Validation enforces global spelling uniqueness
   (`gen_op_kinds.py:300-325`) and cross-set disjointness
   (`_validate_disjoint_opcode_role_sets`).
2. **Render** the `matches!` exact-set + any prefix rule (the `FRESH_VALUE_PREFIXES`
   pattern, `gen_op_kinds.py:1155`). Open domains keep a fail-CLOSED `_ =>` backstop
   (leak-not-UAF — doc 20), but the table makes the *known* set total and the
   producer-drift audit (`audit_op_kinds.py`) makes a new producer kind without a row
   a CI failure.
3-7. Same as 5.1 (replace sites, register, test-at-consumer, coverage row).

### 5.3 The two-tier discipline for any NEW discovery tool
A new ranking probe goes in `structural_audit.py` (or a sibling) marked
`discovery_only = true` in the manifest; it may use regex. Anything that *gates*
behavior consumes the generated artifact. This is doc 46 rule #1, made a checklist
item so it is not re-litigated per tool.

---

## 6. The structural_audit metric tension — the structural fix (F4)

> **Status: LANDED (Phase 0, §8).** The redesign described below has shipped:
> `tools/structural_audit.py` now ratchets `kitchen_sink_files`,
> `undecomposed_god_files`, and `max_undecomposed_file_lines` (concern-mixing + lone-
> monolith signals) and treats raw line count as board-only triage
> (`large_source_file`). As of 2026-06-27 all three ratcheted decomposition metrics are
> `0`. The line/symbol references below are to the *pre-Phase-0* tool and remain for
> historical rationale; the current authority is the live `structural_audit.py`.

### 6.1 The defect, precisely (as it stood pre-Phase-0)
The pre-Phase-0 `probe_god_files` flagged every non-generated source file over a
per-language ceiling (4000 `.rs`, 2500 `.py`). `ratchet_metrics` exposed two scalars that
"may only go DOWN": `god_files` (the *count* of such files) and `max_god_file_lines` (the
single worst). The ratchet test failed if either rose above baseline.

**The collision:** correct decomposition (doc 21) splits one 39K-line god-file into,
say, eight cohesive 5K-line submodules. Each submodule is *structurally better*
(one responsibility, the Lattner ideal) but each is still over the 4000 ceiling, so
the **count rises 1 → 8** and the ratchet goes RED. Measured at authoring time
(2026-06-23): `god_files: 53 → 57`, `max_god_file_lines: 39520 → 41266` — both regressed
*because of* the `fc/` family split and the `cli/` package, while the fact-migration
metrics improved. The `god_files` count metric **penalized the exact work the project's
decomposition program mandates** — which is why Phase 0 (now landed) replaced it.

The two illegal escapes (both forbidden):
- *Re-pin the baseline up* — forbidden by CLAUDE.md ("never re-pin to hide debt") and
  doc 52 §A.2 (ratchets "never lowered"; raising a down-only baseline IS lowering the
  bar).
- *Stop decomposing* — forbidden by doc 21 (the decomposition program) and the
  Lattner mandate.

### 6.2 The structural fix (not a band-aid, not a re-pin, not hiding kitchen-sink debt)
The root cause is that **raw line-count-over-threshold conflates two different
properties**: "this file is too long" (a *size* signal that decomposition legitimately
trades for file *count*) and "this file mixes unrelated concerns / is a kitchen sink"
(the *real* debt, which decomposition *reduces*). Measure the second, not the first.

**Replace the ratchet metrics with concern-aware signals** (keep the human board's
size ranking — it is still useful triage — but change what the `--check` gate
ratchets):

1. **`kitchen_sink_files` (new ratchet, down-only) replaces `god_files`.** A file
   counts as a kitchen sink only if it is over the ceiling AND exhibits *concern
   mixing*, measured structurally (NOT by raw lines):
   - **Rust:** top-level item count over a threshold *combined with* low cohesion —
     concretely, the file declares items spanning ≥N distinct concern clusters. The
     cheap, robust, discovery-grade proxy already available: a file is a kitchen sink
     if `top_level_item_count / 1000 lines` is high (many small unrelated defs, the
     `cli.py` "896 flat top-level defs" shape, doc 21 §1.1) — versus a *cohesive
     large* file that is a few big functions over one domain (the `function_compiler`
     opcode-family files: large but one concern). Use `pub fn`/`fn`/`impl`/`struct`/
     `enum` top-level counts and the ratio to length.
   - **A cohesive decomposition product is CREDITED, not counted:** a file that lives
     in a recognized decomposition package (a sibling-rich directory: `fc/`,
     `visitors/`, `lowering/`, `cli/`, `object/`, the satellite crates) AND is below a
     *generous* cohesive ceiling (e.g. 6000 lines, the doc 21 §3 "no submodule >6000"
     budget) is NOT a kitchen sink — it is the *target state*. The metric reads the
     directory shape: if a directory has ≥K sibling source files of comparable size,
     its members are decomposition products, not god-files.
2. **`max_god_file_lines` → `max_undecomposed_file_lines` (down-only), with
   in-progress residual excluded from REGRESSION.** The worst *monolith that has not
   begun decomposing* must still shrink monotonically — that is real debt. But a
   package `__init__.py` or a residual `function_compiler.rs` that is *actively
   shedding* into a sibling `fc/` family is mid-migration; its transient size must not
   *fail the gate* while the count of its siblings proves decomposition is underway.
   Concretely: a file is exempt from the `max_*` regression test (still REPORTED on
   the board, loudly) when a sibling decomposition directory exists AND the family's
   *aggregate* (residual + siblings) is not larger than the pre-decomposition single
   file. This credits "I split 10K out of the monolith into cohesive files" as
   progress even though a new 5K sibling appeared.
3. **Add `undecomposed_god_files` (down-only) — the HONEST kitchen-sink count that
   does NOT hide debt.** A file is an *undecomposed god-file* if it is over the
   ceiling AND has NO sibling decomposition directory (it is a lone monolith nobody
   has started splitting). This is the number that must go to zero and that a re-pin
   would be caught lowering. `cli/__init__.py` at 41K *with* a populated `cli/`
   package is mid-decomposition (excluded from regression, reported); a hypothetical
   new 10K lone file with no package is an undecomposed god-file (RED). This is the
   precise line between "credit correct decomposition" and "do not hide real
   kitchen-sink debt" the arc must walk.

**Net effect:** splitting a god-file *lowers* `undecomposed_god_files` (the monolith
gains a package and exits the count) and does NOT raise `kitchen_sink_files` (cohesive
products are credited). The ratchet can finally green *as a reward for* correct
decomposition, while a genuine new kitchen sink (lone, concern-mixing, over-ceiling)
still fails it. **The baseline is NOT re-pinned up; it is re-expressed against a
metric that measures the actual debt.** This is a metric *correction* (the measured
thing was wrong), categorically different from a baseline *relaxation* (the bar
lowered) — the doc 52 §A.4 anti-Goodhart distinction.

### 6.3 Verification that the new metric is not a stealth relaxation
- **Unit tests** (extend `tests/test_structural_audit.py`, the §2.2 robustness
  pattern): a synthetic lone 10K concern-mixing file → flagged `undecomposed_god_file`
  + `kitchen_sink`; a synthetic directory of 8 cohesive 5K siblings → flagged NEITHER;
  a synthetic monolith *with* a populated sibling package → excluded from `max_*`
  regression but PRESENT on the board. These tests *prove the metric finds debt*
  (the "a tool that finds nothing must be proven to find nothing" rule,
  `test_structural_audit.py:8`).
- **The honest-debt invariant:** `undecomposed_god_files + kitchen_sink_files` at the
  new baseline must be ≤ the count the *old* metric would have flagged for *lone,
  un-split* files (i.e. the redefinition only re-buckets decomposition products, never
  erases a true monolith). Assert this in the migration commit.
- **Re-pin once, with the explicit justification** that this is a metric correction
  (decomposition products re-bucketed), recording the before/after for every file
  that changed bucket — the doc 52 §A.2 "justify in writing" path, used for a
  *correction*, never for a *relaxation*.

---

## 7. How facts feed the perf ladder (51) and parity (52)

The fact plane is the *substrate*; doc 51's compression ladder and doc 52's parity
oracle are the *products*. The composition (doc 51 §1, §5; doc 46 §4):

- **Perf (doc 51 §5 "fact plane build-out"):** every ladder item is a fact family that
  follows THIS arc's workflow. `CallFacts` (doc 47), `Typed CallableTarget` (#71),
  `ShapeFacts`, `ExceptionRegion`, class-version guards — each is "add a generated
  authority + attach it to the IR + read it in every backend." This arc makes adding
  each one *cheap and drift-proof* instead of a fresh archaeology. The bridge is F6:
  the debt ratchet (down) and `call_fact_coverage` (up) move together, so a new perf
  fact is only "done" when it is generated AND attached AND covered. **Perf depends on
  the fact plane** — you cannot specialize a call you have not recorded a fact about.
- **The "fix the REPRESENTATION not the pass" posture (CLAUDE.md Performance
  Constitution):** when a benchmark is slow, the first question is "which FACT is
  missing from IR?" The fact plane is *where a new fact goes*. Without F1-F3, the
  answer keeps being a pass-local `matches!` that the next pass loses — exactly the
  drift this arc removes.
- **Parity (doc 52 §A.1 parity oracle, §A.2 ratchets):** doc 52 §A.2 lists
  "structural-audit debt counters DOWN … call-fact coverage UP" as hard ratchets and
  §C.3 #13 names "fact-plane completeness as a standing institution" as the 50-year
  deliverable. This arc *is* that institution. The parity oracle (byte-identical vs
  CPython) is unaffected by the machinery, but the *trust* that a fix is structural
  (not a per-test special-case — CLAUDE.md) is enforced by the fact plane: a parity
  fix that adds a hand-classifier is caught by the debt ratchet; a real fix adds a
  table row and the ratchet *rewards* it.
- **Cross-arc dependency direction (binding):**
  - **demos/perf depend on perf-facts depend on THIS plane.** (doc 46 §5 sequencing:
    the op-semantics ladder keeps absorbing hand-classifications; CallFacts is the
    highest-leverage new primitive *on top of* the registry.)
  - **THIS plane depends on the decomposition (21a-e)** only at the metric seam (F4):
    the plane must not penalize decomposition, and decomposition must not be blocked
    by a red ratchet. F4 resolves the bidirectional dependency.
  - **THIS plane is independent of the runtime/memory P0 lane** (finalizer/ownership,
    doc 50/51 §4) — it is lane C (infra/scoreboards, doc 51 §9), never blocking lane A
    (P0 safety). It *accelerates* lane A by making each ownership fact (operand
    ownership, `explicit_release_operand`, terminator ownership — all already in
    `op_kinds.toml`) drift-proof.

---

## 8. Phases (dependency order; each independently landable with green gates)

Each phase is a **complete structural piece** (CLAUDE.md unit-of-work rule). The arc
is lane C; phases are non-overlapping files so multiple agents can run them (§9).

### Phase 0 - COMPLETE: structural_audit metric correction (F4)
- **Landed:** `tools/structural_audit.py` now separates raw large-file board triage
  (`large_source_file`) from ratcheted structural debt (`kitchen_sink_files`,
  `undecomposed_god_files`, `max_undecomposed_file_lines`, plus kitchen-sink region
  pressure). Cohesive sibling-rich decomposition products and residuals with a
  decomposition directory no longer regress CI merely for being over a raw line ceiling.
- **Tests:** `tests/test_structural_audit.py` proves the metric still finds a lone
  concern-mixing file, credits cohesive sibling packages, reports residual files without
  max-undecomposed regression, and preserves the honest-debt union for lone large files.
- **Generated state:** `tools/structural_audit_baseline.json` was regenerated once as a
  metric correction, and `docs/design/foundation/STRUCTURAL_AUDIT_BOARD.md` was
  regenerated from the corrected authority.
- **Gate:** `python3 tools/structural_audit.py --check` and `pytest -q
  tests/test_structural_audit.py` are the closure proof for this phase.

### Phase 1 — The Generator Manifest + meta-gate (F1, F5).
- **Do:** author `tools/generator_manifest.toml` with a row per existing `gen_*.py`
  (§4 table). Build `tools/check_generator_manifest.py` (orphan-generated-file
  detection generalizing `_is_generated`; `check_mode` enforcement; the
  idempotence double-run check; `closed_domains` cross-check) +
  `tests/test_generator_manifest.py`. Cross-link
  `docs/design/foundation/authority_manifest.toml` ↔ the generator manifest (F5).
- **Files (new):** `tools/generator_manifest.toml`,
  `tools/check_generator_manifest.py`, `tests/test_generator_manifest.py`. **Edit:**
  `.github/workflows/ci.yml` (add the meta-gate step next to `gen_op_kinds.py
  --check`, `ci.yml:56`); `tools/structural_audit.py` (consume the manifest's
  generated-file list instead of the `_is_generated` heuristic — an authority replaces
  a heuristic, doc 46 rule #1).
- **Gate:** `python3 tools/check_generator_manifest.py --check` GREEN; the meta-gate
  correctly FAILS on a synthetic orphan generated file and a synthetic
  non-idempotent generator (proven-to-find-debt tests); CI step added.

### Phase 2 — Close the generator-gating holes (F1 enforcement across the family).
- **Do:** for each generator currently lacking a CI `--check` (§4): add `--check` to
  CI; for the four lacking a sync test (`gen_compat_platform_availability`,
  `gen_diff_lanes`, `gen_luau_support_matrix`, `gen_stdlib_module_union`) author the
  sync test (the `tests/test_gen_op_kinds.py` re-render-and-assert pattern) and prove
  idempotence; flip each manifest row `check_mode`/`idempotent`/`sync_test` to true.
  For `gen_protocol.py`, add a `--check` CI step and record `sync_test reason`
  (import-only; byte-identity is not the gate, the composition test is —
  `gen_protocol.py:25-30`).
- **Files:** `.github/workflows/ci.yml`; new `tests/test_gen_*.py` for the four;
  `tools/generator_manifest.toml` (flip flags). NO changes to generator *logic* unless
  a generator proves non-idempotent (then fix the generator — never silence the gate).
- **Gate:** every manifest row has `check_mode = true`; all new sync tests GREEN; the
  meta-gate (Phase 1) now passes with zero `check_mode = false` rows. This is the
  point where **"a generated file with no committed, gated generator" becomes
  unexpressible.**

### Phase 3 — The Closed-Domain Exhaustiveness Auditor as a reusable mechanism (F2).
- **Do:** extract the `[[terminator]]`/`binary_op` exhaustiveness logic
  (`gen_op_kinds.py:888`, `:1043`) into a shared helper consumed by the manifest
  meta-gate for every declared `closed_domain`. Declare `OpCode`, `Terminator`,
  `ast.operator` as the initial closed domains. Verify the rustc exhaustive-match
  invariant is asserted (no-wildcard render) for each.
- **Files:** `tools/check_generator_manifest.py` (the shared exhaustiveness helper);
  `tools/generator_manifest.toml` (`closed_domains`); a unit test that a synthetic
  enum-variant-added-but-table-not scenario FAILS the gate.
- **Gate:** adding a variant to a closed-domain enum without a table row fails the
  meta-gate (synthetic test) AND fails rustc (the real authority); the two agree.

### Phase 4 — Burn down the deletion-candidate board (the ladder this arc enables).
> **First sweep LANDED (verified 2026-06-27): the ratchet reached 0.** The
> authoring-time deletion candidates were migrated into `op_kinds.toml` generated
> predicates and the hand-classified surface is now **empty** — `hand_classified_matches`,
> `handset_classifications`, and `critical_hand_classifications` all read **0** (live
> `structural_audit.py`; independently confirmed by an exhaustive sweep: effects, alias,
> drop-insertion, copy-kind, repr-facts, deforestation, escape, inliner, and
> module-slot-promotion classifiers now each delegate to a generated `*_table()`). The
> *mechanism* (this phase) remains the standing, unbounded cadence for every **future**
> opcode-fact, now drift-proof. The named line anchors below were the pre-migration
> coordinates and are retained for historical context.
- **Do:** this is the *ongoing* lane-C work the institution makes safe — migrate the
  top `STRUCTURAL_AUDIT_BOARD.md` deletion candidates (historically: the 108-opcode
  `lower_to_wasm.rs:551` / `lower_to_simple.rs:1651` near-exhaustive silent-default
  matches; the 74-opcode `verify.rs:235`; the 56-opcode `type_refine.rs:1218`; the
  `effects.rs:313` / `inliner.rs:182` 10-opcode `matches!` sets) into generated
  predicates via the §5 workflow. Each migration deletes a hand-classifier and drops
  `hand_classified_matches` / `handset_classifications` toward 0.
- **Files:** `op_kinds.toml` (+ new columns/sets), `gen_op_kinds.py` (render),
  `op_kinds_generated.rs`, and the per-site `.rs` files (delete the local
  classifier, read the predicate). One fact family per commit (the unit-of-work rule).
- **Gate per migration:** `gen_op_kinds.py --check` GREEN; `cargo test -p
  molt-backend` (byte-diff — the migration must not change behavior); the
  round-trip-at-consumer regression (the fact survives `lower_to_simple` → re-lift);
  `structural_audit --check` shows the target metric DECREASED (the deletion
  candidate is gone). **This phase is unbounded** — it is the monthly compression-
  ladder cadence (doc 51 §1), now drift-proof.

**Landing report format for every phase (doc 51 landing-report + CLAUDE.md PERF/SPEED
block):** "tests green; the named gate(s) green; which ratchet metric moved and by how
much; zero new hand-classifiers introduced; for Phase 4, the byte-diff confirms
behavior-preservation." No phase is done on "looks done" (doc 52 stop conditions).

---

## 9. Composition with the decomposition (21a-e) and the multi-agent model

- **21a-e composition:** Phase 0 is the *enabling* fix for the entire decomposition
  program — it removes the metric that punishes splitting god-files, so the 21a
  (`function_compiler` fc-split), 21c (frontend mixins), 21d (cli package) agents can
  land without a red ratchet or a forbidden re-pin. The decomposition agents and this
  arc are mutually unblocking: this arc fixes the metric; their splits exercise it.
  After Phase 0, `function_compiler.rs` shedding into `fc/*.rs` REDUCES
  `undecomposed_god_files` (the monolith joins a package) instead of inflating a
  count.
- **Generated files are decomposition-neutral:** `op_kinds_generated.rs` (3906 lines)
  and `intrinsics/generated.rs` (~24.5K) are correctly excluded by `_is_generated`
  (`structural_audit.py:87`) — Phase 1 replaces that heuristic with the manifest's
  authoritative list so the exclusion is itself an authority, not a guess.
- **Multi-agent (doc 52 §"Resources & parallelism"; CLAUDE.md):** the phases occupy
  NON-OVERLAPPING files, enabling parallel lane-C agents:
  - Agent X (Phase 0): `tools/structural_audit.py` + its test + baseline. Touches no
    other lane's files.
  - Agent Y (Phases 1-3): `tools/generator_manifest.toml` +
    `tools/check_generator_manifest.py` + the new `tests/test_gen_*.py` + `ci.yml`.
    Touches generators' *gating* (CI/tests), not their *logic*.
  - Agent Z (Phase 4): one fact-family migration at a time —
    `op_kinds.toml`/`gen_op_kinds.py`/`op_kinds_generated.rs` + the per-site `.rs`.
    This is the only build-triggering lane (≤2 build agents, CLAUDE.md); it serializes
    behind the backend daemon, sessions isolated via `MOLT_SESSION_ID`.
  - **Ordering constraint:** Phase 0 lands first (unblocks everyone). Phases 1-3 land
    before Phase 4 *claims drift-proofness* (Phase 4 migrations are safe without the
    manifest, but the manifest is what *guarantees* the next migration cannot
    re-introduce an ungated generator). Agents never push; the lead integrates,
    rebases, re-gates, pushes (doc 52).

---

## 10. Risks + structural (not band-aid) treatment

| risk | where it bites | STRUCTURAL treatment (no band-aid) |
| --- | --- | --- |
| **The new `kitchen_sink`/`undecomposed_god_file` metric is itself gameable** (a kitchen sink dodges detection by adding one cohesive sibling) | Phase 0/F4 | The metric keys on the file's OWN concern-mixing (item/line ratio), not only directory shape; a concern-mixing file stays flagged `kitchen_sink` even inside a package. The honest-debt invariant test (§6.3) asserts the redefinition never *erases* a true monolith, only re-buckets cohesive products. Discovery-grade by design (it RANKS); the authority that a file is "decomposed" is the human review + the doc-21 budget, not the metric alone. |
| **Re-pinning the baseline in Phase 0 looks like the forbidden relaxation** | Phase 0 | It is a metric CORRECTION (the measured quantity was wrong — it conflated size with debt), executed ONCE with a full before/after bucket table and the honest-debt invariant proving no monolith was hidden (doc 52 §A.4 distinction: correcting a wrong measurement vs. lowering a correct bar). Never re-pin again on this metric without the same invariant. |
| **Meta-gate idempotence check has false positives** (rustfmt/version nondeterminism makes a generator's double-run differ) | Phase 1 | The check runs the generator twice in ONE environment and diffs; nondeterminism is then a REAL generator bug (it would also fail the existing `--check` sync test intermittently) and is fixed in the generator (pin the rustfmt invocation, the `RUSTFMT_TMP` discipline, `gen_op_kinds.py:57`), never by exempting the generator. |
| **A generator proves non-idempotent or non-byte-stable in Phase 2** | Phase 2 | The generator is FIXED (deterministic ordering, sorted iteration — the `gen_protocol.py:41` determinism discipline), not granted a manifest exemption. A non-idempotent authority is by definition not an authority. |
| **`_count_enum_variants` parser under/over-counts a closed domain** | Phase 3/F2 | Discovery-vs-authority firewall: the parser is DISCOVERY (ranks "is the table total"); the rustc exhaustive match is AUTHORITY (refuses an unclassified variant at compile time). A parser miss can only yield a false "looks total" — rustc still fails the build. The parser is unit-tested against payloads/discriminants (`test_structural_audit.py:140`); a new enum syntax that breaks it is a test-extension, caught because the test asserts the parser FINDS the variants. |
| **Phase 4 migration changes behavior** (a hand-classifier and the table disagree — the table had a latent bug) | Phase 4 | The byte-diff gate (`cargo test -p molt-backend`, behavior-preservation) catches it; the FINDING (the disagreement) is the deliverable (doc 52 honesty protocol) — it means the hand-classifier OR the table was wrong, and resolving which is a correctness win, not a migration blocker. The fail-CLOSED `_ =>` backstop (leak-not-UAF, doc 20) bounds the worst case to a leak, never a UAF, during the window. |
| **The institution adds ceremony that slows fact-family authors** (the opposite failure: over-process) | F3/ongoing | The workflow (§5) is the *path of least resistance* (add a column + regenerate beats hand-writing a classifier in N files); the meta-gate enforces only what a human would otherwise have to remember. Doc 52 §B.3 "bloated charters get ignored" — the manifest is a 1-row-per-generator table, not a process doc. |
| **A new fact family belongs in a DIFFERENT table** (not `op_kinds.toml`) and the manifest ossifies one table | F1/F5 | The manifest is generator-keyed, not table-keyed — it already accommodates 8 distinct source tables/ASTs. A new fact family with its own table (e.g. a future `shape_facts.toml`) gets its own generator row; F5's authority cross-link points to it. The plane is multi-table by construction (doc 46 §4 lists FactGraph/Region/RuntimeInterface as distinct authorities). |

---

## 11. The single most important sentence

The fact plane's deliverable is **not** a faster compiler or a more correct one — it
is a codebase in which *"two authorities for one invariant"* and *"a new member the
oracle silently defaults"* are **compile errors or red CI**, so that every future
correctness fix and every future perf fact is forced down the one drift-proof path,
and the compression ladder (doc 51) becomes table rows and verifier obligations
instead of heroic debugging (doc 46 §0). Phase 0 unblocks the ratchet that proves it;
Phases 1-3 make the institution total; Phase 4 is the monthly cadence it makes safe.

---

*Design only / executable plan. portfolio-architect, 2026-06-23.*
*Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>*
