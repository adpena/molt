<!-- Foundation blueprint 57. Arc: USER EXPERIENCE — the molt CLI command surface,
the diagnostics/error layer, source-mapped tracebacks across all backends, the
onboarding path, and the documentation system. Author: portfolio-architect.
Date: 2026-06-24 (assigned 2026-06-23). Design only / executable plan.
Composes with: 21d (cli/ package decomposition), 52/parity-tracebacks (future),
58/demos (future), 51 (compression ladder), 52 (autonomous charter).
Assigned number 57 was free; this doc takes it. -->

# 57 — User Experience: CLI Surface, the Diagnostic Authority, Source-Mapped Tracebacks, Onboarding, and Docs

Status: EXECUTABLE PLAN (design only). Lane-C (infra/DX) with one Lane-A-adjacent
correctness vertical (traceback parity). No build performed; read-only investigation
+ this one doc.

---

## 0. The end-state outcome (time-traveler anchor)

**A Python developer installs molt and, within five minutes, has run a real program
faster than CPython with byte-identical behavior — and when something goes wrong, the
error is at least as good as CPython's and usually better: precise source context, a
correct cross-backend traceback, and an actionable fix suggestion. There is zero
friction between "I have a .py file" and "I have a fast, correct binary," and zero
ambiguity when molt refuses something outside the verified subset.**

Concretely, at the 5-to-100-year end-state:

1. **One error authority.** Every diagnostic molt emits — frontend syntax/lowering
   errors, runtime exceptions/tracebacks, CLI/toolchain errors, subset-refusals — is a
   single first-class structured `Diagnostic` value produced by ONE authority and
   rendered by ONE renderer. There is no second place that formats `File "...", line N`,
   no second `eprintln!` that invents its own error shape, no CLI `print(..., file=sys.stderr)`
   that bypasses the renderer. A new error class is *unexpressible* except as a `Diagnostic`.

2. **Source-mapped tracebacks that are a proven fact, not a reconstruction.** The
   `(file, line, col, end_col, function, qualname)` of every Python-visible program point
   survives — losslessly and identically — from the frontend AST through every IR
   boundary (serialization round-trip, TIR, SimpleIR, LIR) to every backend (native,
   LLVM, WASM, Luau). A traceback on the WASM backend is byte-identical to the same
   traceback on native, because both read the SAME source-location fact, not a
   per-backend best-effort. This retires the *class* "the traceback is wrong/missing on
   backend X" rather than fixing backend X.

3. **Better-than-CPython errors as a structured capability.** "Did you mean", the
   PEP-657 fine-grained caret, the offending source line, the chained-exception context,
   and — molt's differentiator — a `Hint` channel (actionable fix suggestions, e.g. "this
   construct is outside the verified subset; the supported form is X" or "`list.appnd` is
   not a method; did you mean `append`?") are all fields of the one `Diagnostic`, sourced
   from the fact plane (CallFacts, class-version/shape, the verified-subset manifest), not
   string-matched after the fact.

4. **A CLI that feels like cargo/uv/go.** Post-21d, the command surface is a clean,
   discoverable, byte-identically-helpful package: predictable verbs, consistent flags,
   `--json` everywhere for tooling, and a `doctor`/`setup` path that gets a new machine to
   green with one command. `molt run app.py` Just Works; `molt explain <error-code>` exists
   for any diagnostic.

5. **Documentation that is generated from the truth.** The CLI reference, the
   diagnostic catalog, and the verified-subset surface are *generated from the code/specs*
   (the argparse tree, the diagnostic registry, the subset manifest) so they cannot drift.
   Prose docs (getting-started, the killer demo, the spec) sit on top of generated
   reference material that is CI-checked against the binary.

The single load-bearing structural decision: **make `Diagnostic` a first-class fact with
one producer authority and one renderer, and make source-location a fact that survives
every IR boundary.** Everything else in this arc is a consequence of, or a consumer of,
that decision. This is the compression-ladder move (doc 51 §1): the recurring root cause
of bad errors is the same as every other molt bug — *the high-level meaning (which source
span, which Python frame, which fix applies) was lost at a low-level boundary and is being
reconstructed afterward.* The cure is the same: carry the fact; never reconstruct it.

---

## 1. Investigation: what exists today (cited, verify-first)

The verify-first pass (CLAUDE.md, charter §"RECON") found that the substrate is
**more built than the docs suggest** in two areas and **structurally absent** in one.

### 1.1 Source location is ALREADY plumbed through the frontend (strong base)

- `src/molt/frontend/lowering/serialization.py` propagates the full line/column
  SourceSite transport (`source_line`, `col_offset`, `end_col_offset`) on ops at
  multiple stages (op-rewrite, copy, `LINE` emit, and the active-line post-pass).
  The Rust side owns the canonical TIR attr wrapper in
  `runtime/molt-ir/src/tir/ops.rs` (`SourceSite` over `_source_line`,
  `_col_offset`, `_end_col_offset`), and SSA/lowering/rewrite passes now inherit
  that fact as one unit instead of carrying columns as ad hoc keys.
- `src/molt/frontend/lowering/op_kinds_generated.py` (generated from
  `runtime/molt-ir/src/tir/op_kinds.toml`, doc 25) carries `RAISING_KIND_NAMES`
  (line 292): the set of op.kinds that can raise, for which `emit()` attaches the caret
  `col_offset` (header lines 16–17, 289). This is a *generated, registry-backed* fact —
  exactly the right shape, already wired to the op-kind single source of truth.
- So: **the frontend already produces PEP-657-grade location facts.** The gap is NOT
  "the frontend doesn't know the span." It is downstream (transport + one authority).

### 1.2 The runtime traceback formatter is real and CPython-shaped (strong base)

- `runtime/molt-runtime/src/builtins/exceptions.rs` (7,229 lines) contains the
  formatter: `format_exception_with_traceback` (3966), which handles **chained
  exceptions** ("direct cause" / "during handling", 3978/3988) recursively;
  `format_single_exception` (3997), which emits `Traceback (most recent call last):`,
  `  File "{file}", line {line}, in {name}`, the trimmed source line, and a PEP-657
  caret via `traceback_format_caret_line_native` (4020, in
  `object/ops_sys.rs`) — gated on `col >= 0 && end_col >= 0` with **no heuristic
  fallback** (4013–4016, the correct CPython behavior).
- `frame_stack_top_info` (4007) and `read_source_line` (4009) already exist; the tb
  object carries `tb_frame`/`tb_lineno` (4218–4252).
- Per-exception message specializations already exist (UnicodeDecode/Encode, HTTPError,
  URLError, ExceptionGroup — 4044–4081), i.e. the message channel is already
  per-class-specialized.
- So: **the runtime already renders CPython-shaped tracebacks with carets and
  chaining.** The gaps: (a) only frames *with* tb objects or the top frame get full
  treatment ("Most module-level exceptions in AOT-compiled code lack traceback objects",
  4002–4005 — i.e. the *interior* frames of a deep stack are the weak point); (b) this
  formatter is ONE of several places that emit `File "..."` text; (c) no `Hint` channel.

### 1.3 "Did you mean" exists in exactly ONE place (the structural gap)

- `runtime/molt-runtime/src/builtins/modules.rs:5179` emits
  `name '{trace_name}' is not defined. Did you mean: '{similar}'?` — the ONLY
  suggestion site in the runtime. There is no shared "nearest-name" / suggestion
  authority; `AttributeError`, `ImportError` (`from M import missing`), keyword-arg
  typos, and method typos do not get suggestions. This is a one-instance fix where a
  *class* mechanism belongs.

### 1.4 Errors are emitted from HUNDREDS of un-unified sites (the core gap)

- `src/molt/cli/__init__.py` (the post-21d package anchor; 21d Phase 0 moved
  `cli.py` → `cli/__init__.py`): **409 lines match error-emission patterns**
  (`print(...Error`, `sys.stderr`, `_fail(`, `raise SystemExit`, `parser.error`) and
  **164 raw `file=sys.stderr` sites**. 21d §2 already designates a `_shared.py` leaf
  holding `_fail`, `_emit_json`, `_json_payload` — so a CLI-side diagnostic funnel has a
  natural home, but today `_fail` is one of many exits, not THE exit.
- Frontend lowering raises Python exceptions (`SyntaxError`, lowering errors) with
  their own formatting; the runtime formats its own; the CLI prints its own. **Three+
  independent error vocabularies, no shared codes, no shared renderer.**
- There is **no `Diagnostic` type, no diagnostic registry, no error-code namespace**
  anywhere in the tree (searched `src/molt`, `runtime/`). `molt-worker/src/diagnostics.rs`
  and `molt-tir/src/process_diagnostics.rs` exist but are narrow (worker JSON + process
  RSS labels), not the user-facing authority.

### 1.5 Docs: navigation-rich, onboarding-thin, drift-prone

- `docs/getting-started.md` (78 lines) is correct but minimal: install → hello → compare
  → bench. No "killer demo," no error-handling showcase, no "why molt" payoff in the
  first run. `docs/cli-reference.md` (24 KB) is hand-maintained — it WILL drift from the
  argparse tree (it already documents `--target mlir` "requires LLVM 22" etc. by hand).
- `docs/INDEX.md` and `docs/spec/README.md` are pure navigation. `docs/spec/STATUS.md`
  (85 KB) is the living state. The **verified-subset contract**
  (`docs/spec/areas/compat/contracts/verified_subset_contract.md`, INDEX line 86) exists
  but is not wired to the error path (a subset refusal does not point the user at it).
- No generated CLI reference, no generated diagnostic catalog. Doc 52 §C.3 item 12 names
  the verified subset becoming "a formal, machine-checkable artifact" as the 50-year
  prize — the error path is where that artifact becomes *visible to the user*.

### 1.6 What this means

The substrate (frontend location facts + runtime caret formatter) is ~70% of a great
error story already built — but it is **uncoordinated**, **lossy at interior frames and
IR boundaries**, and **has no single authority, no hint channel, and no codes**. The
correct structural move is NOT to improve any one formatter. It is to introduce the
**`Diagnostic` fact + the `SourceMap` transport fact**, route every existing emitter
through them, and *delete* the ad-hoc sites. That retires the class.

---

## 2. The structural facts/mechanisms to build (each tied to the class it retires)

This arc adds **two fact families** and **one institution**, mirroring the doc 51 fact-plane
method. Each is a producer + transport (round-trip-tested!) + consumer + a test at each
layer (charter §loop step 4: "facts silently die at REPRESENTATION BOUNDARIES").

### Fact F-DIAG — `Diagnostic`: the one structured error value (the keystone)

- **What it is.** A single structured type, defined ONCE in Rust
  (`runtime/molt-diagnostics/` — a new leaf crate, see §5.1) and mirrored ONCE in Python
  (`src/molt/diagnostics/` — a strict-leaf package), with a stable wire form
  (JSON/msgpack, reusing the existing codec the CLI already speaks). Fields:
  - `code: DiagCode` — a stable enum/namespaced string (e.g. `MOLT-RT-NameError`,
    `MOLT-FE-Syntax`, `MOLT-SUB-Unsupported`, `MOLT-CLI-Toolchain`). The namespace is
    `MOLT-<layer>-<class>`; codes are GENERATED from a registry (`diagnostics.toml`,
    like op_kinds.toml) so producer + renderer + docs share one spelling (doc 25 pattern).
  - `severity: Error | Warning | Note | Hint`.
  - `primary_span: Option<SourceSpan>` + `secondary_spans: Vec<(SourceSpan, label)>` —
    the PEP-657 spans (file, line, col, end_line, end_col), reusing §1.1's facts.
  - `message: String` (the CPython-parity message; byte-identical where parity demands).
  - `hints: Vec<Hint>` — the *better-than-CPython* channel: each hint is structured
    (`DidYouMean { candidates }`, `SubsetAlternative { supported_form, manifest_ref }`,
    `FixIt { span, replacement }`, `SeeDoc { anchor }`). Sourced from the fact plane,
    never string-heuristic.
  - `chain: Vec<Diagnostic>` (cause/context, replacing the recursive string-building in
    `exceptions.rs:3966`).
  - `traceback: Option<Traceback>` (a `Vec<FrameSummary>`, see F-SRCMAP).
- **Class it retires.** "Every error is shaped differently and formatted in a different
  place" → made *unexpressible*: you cannot emit a user-facing error except by
  constructing a `Diagnostic` and handing it to the renderer. This is the
  doc 49 "no second authority for any fact" rule applied to errors.
- **Producer authority.** ONE constructor surface per layer feeding the SAME type:
  - runtime: `exceptions.rs` builds `Diagnostic` from a live exception object (it already
    has the data — 4028–4034 message, 4007 frame info, 4016 caret).
  - frontend: lowering raises that carry `Diagnostic` (SyntaxError, unsupported-construct)
    instead of bare Python exceptions with ad-hoc text.
  - CLI: `_shared.py:_fail` (21d) becomes `emit_diagnostic(diag)` — the ONE CLI exit.
- **Renderer authority.** ONE function `render(diag, style) -> String` (Rust) +
  its Python mirror, with `style ∈ {Human, Json, Short}`. Human style is byte-identical
  to CPython for the traceback+message portion (the parity oracle, §6) and APPENDS hints
  below (hints never perturb the parity-checked region — they live after the
  `Error: message` line, like rustc's `help:` notes).

### Fact F-SRCMAP — `SourceMap` / `FrameSummary`: source location that survives every boundary

- **What it is.** A first-class, per-function (then per-op) location table carried in the
  IR and emitted into the artifact, such that any runtime program-counter / frame can be
  resolved to `(file, line, col, end_col, function, qualname)`. Today the *frontend*
  has this (§1.1) and the *top runtime frame* has it (§1.2), but it **dies at the IR
  boundaries** for interior frames ("Most module-level exceptions ... lack traceback
  objects", `exceptions.rs:4002`). The fact must round-trip: frontend op spans →
  TIR (carried as an op attribute, like the existing exception-label attrs in op_kinds.toml)
  → SimpleIR → LIR → a **per-backend source-map artifact** (native: a side table keyed by
  return-address/frame-id; WASM: the names/DWARF-line custom section; LLVM: debug-loc;
  Luau: line-mapped source).
- **Class it retires.** "The traceback is wrong, truncated, or missing on backend X / for
  interior frames / after optimization (inlining)." Made unexpressible because the
  location is a carried fact validated AT the consumer (the renderer), with a round-trip
  test at every boundary. Note the inlining interaction: the E1 inliner (doc 01,
  `7512919fa`) and generator fusion MUST preserve/compose source spans (an inlined frame
  needs an inline-stack, like CPython 3.11+/rustc) — this is called out as a risk (§7) and
  is exactly the "fact dies at a boundary" landmine the charter warns about.
- **Producer.** The frontend span post-pass (`serialization.py:4405`) extended from
  "raising kinds get caret cols" to "every op carries its `SourceSpan` + owning
  function/qualname," gated by the op_kinds registry (`RAISING_KIND_NAMES` generalized to a
  `carries_span` column in `op_kinds.toml`).
- **Transport.** The current line/column site travels as a typed TIR op attribute
  through the SimpleIR/TIR round-trip. The remaining target is to extend this
  carried fact from line/column SourceSite into full span + function identity and
  lower it into each backend's native debug-info channel.
- **Consumer.** The runtime traceback walker resolves frames against the emitted source
  map (replacing the "only top frame / only tb-bearing frames" limitation), and the
  renderer (F-DIAG) consumes the resolved `Vec<FrameSummary>`.

### Institution I-DIAGCAT — the diagnostic catalog + the suggestion authority + generated docs

- **`diagnostics.toml`** (a registry, like `op_kinds.toml`): every `DiagCode` with its
  message template, its default hints, and its doc anchor. Generates: the Rust `DiagCode`
  enum, the Python mirror, and **`docs/diagnostics/` (one page per code)** — so
  `molt explain MOLT-SUB-Unsupported` and the docs site read the SAME source. This is the
  doc 52 §C.3-#12 "verified subset as machine-checkable artifact" made user-visible: a
  subset-refusal `Diagnostic` carries a `SubsetAlternative` hint whose `manifest_ref`
  points into the generated catalog, which is itself derived from
  `verified_subset_contract.md`'s witnesses.
- **The suggestion authority** (`runtime/molt-diagnostics/src/suggest.rs` + Python mirror):
  ONE nearest-name / fix-it engine (Damerau-Levenshtein over the relevant namespace —
  locals for NameError, attributes for AttributeError sourced from class-version/shape
  facts, keyword names for call-binding from CallFacts, method names from the MRO). The
  single `modules.rs:5179` site is migrated to consume it; AttributeError/keyword/import
  sites are ADDED as consumers in the same arc (symmetry rule: do not fix NameError-only
  and leave the others — CLAUDE.md "asymmetric coverage" anti-pattern).
- **Class it retires.** "Suggestions are ad-hoc, exist in one place, and differ in
  quality." Made a uniform capability of the `Hint` channel.

### How the three compose

`F-SRCMAP` is the transport that `F-DIAG` consumes for tracebacks; `I-DIAGCAT` is the
registry/institution that defines `F-DIAG`'s codes/hints and generates the docs. The
dependency order is therefore: F-DIAG type + registry (the spine) → F-SRCMAP transport
(fills the traceback) → suggestion authority + catalog docs (fills the hints) → CLI/docs
consumers. This is the phase order in §4.

---

## 3. Cross-arc dependency map (compose, never duplicate)

| This arc needs | From | Status / citation |
|---|---|---|
| `cli/` package with `_shared.py` leaf + `__init__` dispatch | **21d** | 21d §2 target layout; `_shared.py` holds `_fail`/`_emit_json`. This arc turns `_fail` INTO the diagnostic funnel. **Ordering: F-DIAG CLI consumer lands AFTER 21d Phase 1 (`_shared.py` extracted).** |
| op_kinds registry pattern (toml → generated producer+consumer) | **doc 25** (`25_op_kind_registry.md`) | `diagnostics.toml` and the `carries_span` column reuse this exact generation discipline; `tests/test_gen_op_kinds.py` is the template for `test_gen_diagnostics.py`. |
| Source spans already on raising ops | **frontend F2** (doc 44, `op_kinds_generated.py:RAISING_KIND_NAMES`) | F-SRCMAP generalizes `RAISING_KIND_NAMES` → `carries_span`; composes with 21c frontend mixin decomposition. |
| Existing caret formatter + chaining | runtime `exceptions.rs` | Refactored to BUILD `Diagnostic` rather than format strings inline; the parity behavior is preserved as the oracle. |
| Inliner / generator-fusion span preservation | **doc 01** (E1, `7512919fa`), **doc 07** (D1) | F-SRCMAP must add inline-stack carrying; this is a *consumer obligation on the inliner*, flagged as the top risk (§7). Coordinate file-lane with whoever owns the inliner. |
| Verified-subset witnesses | `verified_subset_contract.md`, doc 52 §C.3-#12 | The `SubsetAlternative` hint + generated catalog derive from these; this arc makes the subset *visible at the error*, advancing the 50-year manifest goal. |
| Parity-tracebacks arc (future "doc 52/parity") | sibling | The prompt names a future parity-traceback doc. THIS doc supplies the *structural substrate* (F-SRCMAP + F-DIAG renderer with a parity-locked region); that arc supplies the *exhaustive differential corpus*. They compose: substrate here, corpus there. If that doc lands first, this arc consumes its corpus as the §6 oracle. |
| Demos arc (future doc 58) | sibling | The "killer demo" onboarding path (§4 Phase 5) is the demo arc's *delivery surface*; this arc builds the onboarding scaffold (`molt new`, the first-run payoff, error showcase), doc 58 fills the demo content. Cross-link, don't duplicate. |
| Cold-start (#62) | doc 51 §5, doc 52 §closing | First-run latency is part of "zero friction"; this arc does NOT own cold-start (it's an artifact-footprint arc) but the onboarding demo MUST measure and surface it honestly (no warm-only claims — charter perf discipline). |
| Perf scoreboards | CLAUDE.md Perf Constitution | The CLI is the surface that *renders* the scoreboards (`molt bench`, `compare`); this arc keeps the rendering honest (cold AND warm, classified GREEN/RED/TIE/DIMENSIONAL_WIN per the charter). |

**This arc does not contradict any existing doc.** It adds the diagnostics fact plane that
docs 51/52 named as needed (51 §4.D "#53 caret coverage"; 52 §C.3-#12 subset manifest
visibility) but that no current doc owns. It explicitly *defers to* 21d for the package
shape and to the future parity/demo docs for corpus/content.

---

## 4. Phases in dependency order (each independently landable with green gates)

Each phase is a COMPLETE structural piece (CLAUDE.md "structural change as the unit of
work"); none ships a half-fact. Phases are sized to be one-agent units and to parallelize
across non-overlapping file lanes (charter §parallelism) where noted.

### Phase 0 — Inventory + the parity oracle (no behavior change; gate-capture)

**Goal.** Before touching any emitter, freeze the current error behavior as the oracle, so
every later phase proves "no parity regression."

- Enumerate every user-facing error site: the 409 CLI sites (§1.4), the runtime
  `format_*` family (`exceptions.rs`), the frontend lowering raises. Produce
  `docs/design/foundation/57a_diagnostic_site_inventory.md` (a generated table, like the
  21d surface probe) — this is the migration checklist (symmetry guarantee).
- Build the **parity oracle harness**: a corpus of ~60 programs that each trigger a
  distinct error class (NameError, AttributeError, TypeError, ZeroDivisionError, deep
  traceback, chained exception, SyntaxError, import error, subset refusal), run under
  BOTH CPython and molt on ALL backends (native/LLVM/WASM/Luau), capturing
  stderr byte-for-byte into `tests/differential/diagnostics/`. This is the un-gameable
  oracle (charter §A.1). Today's molt output IS the baseline; the goal of later phases is
  "byte-identical traceback+message region, hints appended below."
- **Gate.** Harness runs green (captures baselines on all backends); inventory table
  committed; zero code behavior change. `pytest tests/differential/diagnostics/ -k baseline`.

### Phase 1 — F-DIAG spine: the `Diagnostic` type, registry, and renderer (the keystone)

**Goal.** Introduce the one type + one renderer + the registry, with the runtime traceback
formatter refactored to PRODUCE `Diagnostic` and the renderer reproducing today's output
byte-for-byte (proven against Phase 0's oracle). No new behavior yet — pure structural
unification of the runtime path.

- New leaf crate `runtime/molt-diagnostics/` (depends only on std + the codec crate;
  NEVER on molt-runtime/backend — strict leaf, like `_shared.py` / `frontend/_types.py`):
  `Diagnostic`, `DiagCode`, `SourceSpan`, `Hint`, `FrameSummary`, `Traceback`,
  `render(diag, style)`.
- `diagnostics.toml` + `tools/gen_diagnostics.py` → generated `DiagCode` enum (Rust) +
  `src/molt/diagnostics/_codes.py` (Python mirror) + `tests/test_gen_diagnostics.py`
  (sync pin, mirroring `test_gen_op_kinds.py`).
- Refactor `exceptions.rs::format_single_exception` / `format_exception_with_traceback`
  to build a `Diagnostic` (message from 4028, frames from 4007, caret span from 4016,
  chain from 3972–3992) and call `render(diag, Human)`. The OLD string-building is
  DELETED, not left as a parallel path (no second authority).
- **Gate.** Phase 0 oracle byte-identical on ALL backends (the refactor is behavior-
  preserving by construction); `cargo test -p molt-diagnostics`; `cargo test -p
  molt-runtime --lib`; clippy clean; `tests/differential/diagnostics/` green vs baseline.
  Perf: zero hot-path cost (error path only) — confirm no `Diagnostic` allocation on the
  success path (a debug-assert that the constructor is never reached without a live
  exception).

### Phase 2 — F-SRCMAP transport: interior-frame + cross-backend source maps

**Goal.** Make the source-location fact survive every IR boundary so interior frames and
every backend get correct `File "...", line N, in name` — retiring the "wrong/missing
traceback on backend X / interior frame" class.

- `op_kinds.toml`: generalize `RAISING_KIND_NAMES` to a `carries_span` column +
  add per-function `qualname`/`co_filename` identity to the function fact. Regenerate
  both `op_kinds_generated.{rs,py}`.
- Frontend (`serialization.py:4405` post-pass): emit the full `SourceSpan` +
  owning-function identity on every span-carrying op (not just raising ones), so the IR
  carries a complete line table.
- Transport: carry the span as a TIR op attribute through the serialization round-trip
  (round-trip test: a span on an op survives `serialize → deserialize` identically — the
  charter §loop-step-4 "test the fact AT the consumer" rule); then lower into each
  backend's debug channel:
  - native (Cranelift): a side table mapping frame-id/return-site → `FrameSummary`.
  - WASM: the `name` custom section + line table.
  - LLVM: debug-loc metadata.
  - Luau: line-mapped emission.
- Runtime walker: resolve interior frames against the emitted map (replacing
  `exceptions.rs:4002`'s "only top/tb-bearing frames" limitation).
- **Gate.** A deep-stack (≥5 interior frames) traceback is byte-identical to CPython on
  ALL FOUR backends (this is a NEW capability, so the oracle for these cases is CPython
  directly, added to `tests/differential/diagnostics/`). Round-trip span test green.
  This is the **one Lane-A-adjacent correctness vertical** (traceback parity is a parity-
  oracle obligation, charter §A.1) — treat divergence as P0.
- **Parallelism note.** The four backend lowerings are non-overlapping file lanes →
  parallelizable across agents after the TIR-transport piece lands, but each backend's
  parity must be proven before that backend's sub-phase is "done" (no asymmetric landing).

### Phase 3 — The suggestion authority + the Hint channel (better-than-CPython)

**Goal.** One nearest-name/fix-it engine; migrate the single existing site and ADD the
missing consumers; wire hints into the renderer.

- `runtime/molt-diagnostics/src/suggest.rs` (+ Python mirror): Damerau-Levenshtein
  nearest-match over a supplied candidate set, with CPython's threshold rules for parity
  where CPython itself suggests, and ADDED suggestions where CPython does not (appended as
  `Hint`, below the parity region).
- Consumers (ALL in this one arc — symmetry): NameError (migrate `modules.rs:5179`),
  AttributeError (candidates from class-version/shape facts), call keyword typos
  (candidates from CallFacts arity/kwnames), method typos (candidates from the MRO),
  `from M import missing` (candidates from the module's exported names).
- Renderer: render `Hint`s as rustc-style `help:` / `note:` lines AFTER the
  `Error: message` line, so the CPython-parity region is untouched.
- **Gate.** Where CPython suggests, byte-identical; where molt adds a hint, the parity
  region is unchanged and the hint is asserted present (`tests/differential/diagnostics/`
  + dedicated hint tests). All four new consumer classes covered.

### Phase 4 — CLI diagnostic funnel + `molt explain` (post-21d consolidation)

**Goal.** Route every CLI error through the one renderer; delete the ad-hoc sites; ship
`molt explain <code>` and `--json` diagnostics everywhere.

- `cli/_shared.py:_fail` (21d) → `emit_diagnostic(diag, style)`: the ONE CLI error exit.
  Migrate the 409 sites per the Phase-0 inventory checklist; DELETE each ad-hoc
  `print(..., file=sys.stderr)` / `raise SystemExit(msg)` as it migrates (no parallel
  path). Toolchain/doctor errors become `MOLT-CLI-*` diagnostics with `FixIt`/`SeeDoc`
  hints (e.g. "wasm-ld not found → install via X", pointing at the generated catalog).
- `molt explain <DiagCode>` reads the generated catalog (I-DIAGCAT); `--json` on any
  command emits the `Diagnostic` wire form.
- **Gate.** `diff -r` of CLI `--help`/exit-codes vs the 21d oracle is EMPTY (this arc must
  not perturb 21d's byte-identical-help invariant — coordinate so this lands AFTER 21d
  completes, or on top of it with the same gate). Every migrated site covered by a test;
  `molt explain` round-trips every code in the registry; `pytest tests/cli/`.

### Phase 5 — Onboarding scaffold + generated docs (the friction-zero path)

**Goal.** Make install → first run → payoff frictionless and make the reference docs
generated-from-truth (drift-proof).

- `molt new <name>` (a project scaffold: a `.py`, a `molt.toml` if any, a one-line "now
  run `molt run`") and a strengthened `molt doctor` whose failures are `MOLT-CLI-*`
  diagnostics with fix-it hints (the "get to green in one command" path).
- **Generated CLI reference**: a `tools/gen_cli_reference.py` that walks the argparse tree
  (the parser is kept WHOLE in `cli/__init__` per 21d §3.4) and emits
  `docs/cli-reference.md`, CI-checked against the binary (replaces the hand-maintained
  drift-prone file). **Generated diagnostic catalog**: `docs/diagnostics/` from
  `diagnostics.toml`.
- Rewrite `docs/getting-started.md` around a **payoff-first first run** (the demo arc /
  doc 58 supplies the actual killer demo; this phase supplies the scaffold + the
  error-quality showcase) and link the verified-subset surface from every subset-refusal
  diagnostic.
- **Gate.** `molt new` → `molt run` works on a clean checkout (CI smoke); generated CLI
  reference matches the binary (CI diff gate); generated catalog covers every code;
  getting-started's commands all execute green in CI. Cold-start of the demo measured and
  reported honestly (cold AND warm, per charter).

### Phase sequencing summary

```
P0 oracle  ─┬─→ P1 F-DIAG spine ─┬─→ P2 F-SRCMAP transport ─┐
            │                    └─→ P3 suggestion authority ─┼─→ P4 CLI funnel ─→ P5 onboarding+docs
  (gate-    │                                                 │      (after 21d)
   capture) └────────────── 21d (package shape) ─────────────┘
```
P1 is the keystone (nothing else is expressible without the type). P2 and P3 are
independent consumers of the P1 spine and can run in parallel (non-overlapping lanes:
P2 = IR/backends, P3 = suggest engine). P4 depends on 21d completion + P1. P5 depends on
P4 (it documents the consolidated surface) and cross-links doc 58.

---

## 5. Implementation surface (file paths, types, fns — concrete)

### 5.1 New crate `runtime/molt-diagnostics/` (strict leaf)

```
runtime/molt-diagnostics/
  Cargo.toml                # deps: serde + the existing codec crate ONLY; no molt-runtime/backend
  src/lib.rs                # pub: Diagnostic, DiagCode, Severity, SourceSpan, Hint,
                            #   FrameSummary, Traceback, render(&Diagnostic, Style) -> String
  src/codes.rs              # @generated by tools/gen_diagnostics.py from diagnostics.toml
  src/render.rs             # the ONE renderer; Human style parity-locked, Hints appended
  src/suggest.rs            # Damerau-Levenshtein nearest-match; takes candidate sets
  tests/render_parity.rs    # unit-level renderer parity (Phase 1)
  tests/suggest.rs          # nearest-match unit tests (Phase 3)
```
Workspace wiring: add to the runtime workspace members; `molt-runtime` depends on
`molt-diagnostics` (one-directional, leaf). This composes with doc 21b crate-graph
blueprint (a new leaf crate has no cycle — verify-first confirmed the diagnostics concern
has no inbound deps today).

### 5.2 Python mirror `src/molt/diagnostics/` (strict leaf)

```
src/molt/diagnostics/
  __init__.py     # Diagnostic, render(), emit_diagnostic() — the Python-side authority
  _codes.py       # @generated mirror of codes.rs (test_gen_diagnostics.py pins sync)
  _suggest.py     # mirror of suggest.rs for frontend-side suggestions
```
Strict leaf (stdlib + molt.compat only; never imports molt.cli/molt.frontend internals)
so both the frontend and the CLI can import it without cycles (mirrors 21d `_shared.py`
and `frontend/_types.py` cycle-breaker discipline).

### 5.3 Registry + generators

```
runtime/molt-tir/src/tir/diagnostics.toml   # the DiagCode registry (co-located with op_kinds.toml)
tools/gen_diagnostics.py                     # toml -> codes.rs + _codes.py + docs/diagnostics/*
tools/gen_cli_reference.py                   # argparse tree -> docs/cli-reference.md (Phase 5)
tests/test_gen_diagnostics.py                # sync pin (mirror of test_gen_op_kinds.py)
```

### 5.4 Touch points in existing files (consumers, by phase)

- P1: `runtime/molt-runtime/src/builtins/exceptions.rs` (3952–4081 region → build
  `Diagnostic`); `runtime/molt-runtime/src/object/ops_sys.rs`
  (`traceback_format_caret_line_native` → feed the renderer, not inline strings).
- P2: `src/molt/frontend/lowering/serialization.py` (4405 post-pass → full span);
  `runtime/molt-ir/src/tir/op_kinds.toml` (`carries_span` column); the four backend
  lowerings (`native_backend/`, `llvm_backend/lowering.rs`, `wasm.rs`, `luau.rs` — debug
  channels).
- P3: `runtime/molt-runtime/src/builtins/modules.rs:5179` (migrate);
  `.../builtins/attributes.rs`, `.../call/*` (add AttributeError/keyword consumers).
- P4: `src/molt/cli/_shared.py` (`_fail` → `emit_diagnostic`); `src/molt/cli/__init__.py`
  (the 409 sites, per the P0 checklist); `cli/maintenance.py` (`doctor`/`setup`).
- P5: `docs/getting-started.md`, `docs/cli-reference.md` (now generated), `docs/INDEX.md`
  (add the diagnostics + onboarding entries), `docs/diagnostics/` (generated).

---

## 6. Verification & gates per phase (measurement discipline)

Per the charter (§A.1 hard invariants, §B.3 done-contracts) and CLAUDE.md Perf
Constitution. **The parity oracle is byte-identical stderr vs system CPython** on the
diagnostics corpus, on every backend × profile.

| Phase | Done-contract (pre-registered) | Gate command(s) |
|---|---|---|
| P0 | Oracle harness captures baselines on all 4 backends; site inventory complete; zero behavior change | `pytest tests/differential/diagnostics/ -k baseline`; inventory table committed |
| P1 | `Diagnostic` is the sole runtime error producer; renderer byte-identical to P0 baseline; registry generated+pinned | `cargo test -p molt-diagnostics`; `cargo test -p molt-runtime --lib`; `pytest tests/test_gen_diagnostics.py`; `tests/differential/diagnostics/` == baseline on all backends; clippy `-D warnings` |
| P2 | Deep-stack + interior-frame tracebacks byte-identical to **CPython** on native/LLVM/WASM/Luau; span survives serialization round-trip; inliner inline-stack preserved | `pytest tests/differential/diagnostics/ -k traceback` (all 4 backends); span round-trip unit test; an inlined-frame traceback test |
| P3 | Where CPython suggests → byte-identical; where molt adds a hint → parity region unchanged + hint asserted; all 4 new consumer classes covered | `cargo test -p molt-diagnostics --test suggest`; `pytest tests/differential/diagnostics/ -k suggest` |
| P4 | Every CLI error routes through `emit_diagnostic`; all 409 sites migrated+deleted; `--help`/exit-code `diff -r` vs 21d oracle EMPTY; `molt explain` covers every code | `diff -r` 21d help oracle; `pytest tests/cli/`; `molt explain` round-trip over registry |
| P5 | `molt new`→`molt run` green on clean checkout; generated CLI reference matches binary; generated catalog complete; getting-started commands execute; cold+warm first-run measured | CI smoke (`molt new`); CLI-reference diff gate; catalog-coverage test; getting-started CI run |

**Cross-cutting gates (every phase):** clippy/ruff clean on touched surface; no new
benchmark regression (error path is cold, but the per-op `carries_span` attribute in P2
adds IR size — measure binary-size + compile-time deltas and report them, charter
methodology: a `DIMENSIONAL_WIN`/regression on size is reported honestly, never hidden).
Run the relevant gates for the touched surface; full gates before integration; list any
omitted gate with its reason (charter §gates). The agent's self-assessment is inadmissible
(charter §A.4) — only the reproducible command output counts.

**Perf note specific to P2 (the one with hot-path risk).** Carrying a `SourceSpan` on
every op grows the IR and the artifact (the source-map side table). This is the ONE place
this arc can move a perf/size number. Mitigations, in order of structural correctness:
(a) the span is a compact `(u32 line, u32 col, u32 end_col, FuncId)` packed table, not a
per-op String; (b) the table is a side array indexed by op-id, not inline in the op (so
the optimizer's hot op-walk is untouched); (c) in `release-output` the span table can be
emitted to a separate artifact section that is page-fault-lazy (only read when a traceback
is actually formatted), so warm compute pays nothing. Report binary-size + compile-time on
all backends/profiles; if a size regression appears, the fix is the side-table/lazy-section
representation, NOT dropping the fact (that would re-open the class).

---

## 7. Risks + structural (not band-aid) treatment

| Risk | Why it bites | Structural treatment (no band-aid) |
|---|---|---|
| **R1: Source spans die at the inliner / generator-fusion boundary** (the #1 risk — exactly the charter's "facts die at representation boundaries"). E1 (`7512919fa`) inlines callees; a naive span-carry makes an inlined frame report the WRONG function. | Inlining/fusion rewrite ops without an inline-stack → traceback shows the caller's name for callee code, or omits the inlined frame. | F-SRCMAP carries an **inline-stack** (`Vec<FrameSummary>` per op, like CPython 3.11+ / rustc), and the inliner is given a *consumer obligation*: when it splices a callee, it pushes the callee frame onto the op's inline-stack. A round-trip + a differential test (`apply(inlined_fn)` traceback == CPython) gate it. Coordinate the file lane with the inliner owner (doc 01); do NOT land P2 backend lowering without the inline-stack or it silently miscompiles tracebacks. |
| **R2: The renderer's "Human" style drifts from CPython** (parity-oracle violation, P0). | Any edit to `render()` can perturb the byte-exact traceback region. | The parity-checked region (`Traceback...` through `Error: message`) is a **separate, frozen code path** with the Phase-0 oracle as a CI gate; Hints render in a STRICTLY-AFTER region that the oracle ignores. A change to the parity region requires updating the oracle with an explicit CPython-version-justified diff (charter §A.4 test-immutability). |
| **R3: 21d and this arc both touch `cli/`** → merge/order collision (CLAUDE.md "never trample partner work"). | P4 migrates the 409 sites; 21d is moving those same files. | **Hard ordering: P4 lands AFTER 21d completes** (or rebases onto it), and re-runs 21d's exact `diff -r` help/exit-code oracle. P1–P3 (runtime/frontend/new-crate) are non-overlapping with 21d and proceed in parallel. The `_shared.py`→`emit_diagnostic` change is the single integration point, flagged as its own commit (never folded into a 21d move). |
| **R4: Two diagnostic authorities (Rust + Python mirror) drift** (doc 49 "no second authority"). | The frontend (Python) and runtime (Rust) each need to construct/render diagnostics. | The registry (`diagnostics.toml`) is the SINGLE source; BOTH `codes.rs` and `_codes.py` are GENERATED and pinned by `test_gen_diagnostics.py` (the op_kinds discipline, doc 25). The wire form is one schema; a round-trip test (Rust emit → Python parse and vice versa) gates drift. The renderer logic is duplicated by necessity but covered by a shared golden-output corpus so they cannot diverge silently. |
| **R5: Suggestion engine produces WRONG/insecure hints** (e.g. leaks a name that shouldn't be suggested, or suggests a private). | A naive nearest-match over all names could surface internals or mislead. | The candidate SET is sourced from the SAME fact plane the lookup uses (locals for NameError, MRO-visible attrs for AttributeError, CallFacts kwnames) — never an unrestricted namespace scan. Threshold matches CPython where CPython suggests. Hints are advisory `Note`s, never auto-applied (no silent code mutation — CLAUDE.md). |
| **R6: Generated docs (CLI ref/catalog) drift from the binary** anyway. | A generator that runs out-of-band can lag. | The generators run in CI with a **diff gate**: `tools/gen_cli_reference.py --check` fails the build if the committed file != freshly generated (the standard "generated file is pinned" pattern already used for op_kinds and the support matrices). The docs are derived, not authored. |
| **R7: Scope creep into the parity/demo sibling arcs.** | "Better errors" and "killer demo" are large; easy to over-build. | Hard boundaries (§3): this arc owns the SUBSTRATE (F-DIAG, F-SRCMAP, suggestion authority, CLI funnel, onboarding scaffold, generated docs). The exhaustive traceback DIFFERENTIAL CORPUS belongs to the parity arc; the killer-demo CONTENT belongs to doc 58. This arc cross-links and consumes them; it does not duplicate them. Don't gold-plate (CLAUDE.md). |

---

## 8. How it composes with the decomposition (21a–e) and the multi-agent model

- **21d (cli/ package)** is the *precondition* for Phase 4: the `_shared.py` leaf is where
  `emit_diagnostic` lives, and the whole-parser-in-`__init__` invariant (21d §3.4) is the
  gate P4 must preserve. P1–P3 are disjoint from 21d's files, so they run concurrently.
- **21c (frontend mixin decomposition)** touches `src/molt/frontend/` — P2's
  `serialization.py` span post-pass and the Python suggestion mirror must coordinate
  file-lanes with 21c (non-overlapping functions; the span post-pass is already a
  self-contained block at `serialization.py:4405`).
- **21b (crate graph)** — the new `molt-diagnostics` leaf crate slots into the crate graph
  with no cycle (verify-first confirmed no inbound deps to a diagnostics concern today);
  it composes with, and is a clean example of, 21b's "leaf crate" discipline.
- **21a (function_compiler split)** and the DX crate-extraction (dx_baseline §4/§8) are
  orthogonal — P2's native debug channel is a side table, not a `compile_func_inner` edit.
- **Multi-agent model** (charter §parallelism; ≤3 agents, non-overlapping lanes, ≤2
  build-triggering): the natural lane split is **Agent-D1 = the spine** (P0+P1, new crate
  + runtime refactor — build-triggering), **Agent-D2 = F-SRCMAP backends** (P2, after the
  TIR-transport piece, parallel across the four backend lowerings — build-triggering, so
  serialize with D1 through the daemon), **Agent-D3 = suggestion authority + Python mirror
  + docs generators** (P3 engine + P5 generators — mostly Python, low build cost). P4 is
  the integration step the lead does after 21d, not a parallel lane. Each agent gets the
  pre-registered done-contract from §6 and the refusal licence (charter §B). Agents never
  push; the lead integrates serially, rebases, re-gates, pushes-by-ref.

---

## 9. The compression-ladder statement (what class this arc retires)

This arc retires **the entire class "molt's user-facing communication is reconstructed
ad-hoc at each boundary and is therefore inconsistent, lossy, or worse than CPython."**
After it lands:

- You cannot emit a user-facing error except as a `Diagnostic` (one authority).
- A traceback cannot be wrong on one backend and right on another (one carried
  source-location fact, validated at the renderer, round-tripped at every boundary).
- A suggestion cannot exist in one place and be missing in four others (one suggestion
  authority feeding the one `Hint` channel).
- The CLI reference and diagnostic catalog cannot drift from the binary (generated,
  CI-diff-gated).
- The verified subset cannot be invisible at the point of refusal (the subset-refusal
  `Diagnostic` carries a `SubsetAlternative` hint into the generated catalog — advancing
  doc 52 §C.3-#12's 50-year manifest goal).

In the doc-51 cadence: this is ~one class/month of UX wrongness made *unexpressible*, on
the same fact-plane method as every other molt arc. The deliverable is not "nicer error
strings" — it is **two new IR/runtime facts (Diagnostic, SourceMap) and one institution
(the diagnostic catalog + suggestion authority)** that make a whole class of bad
user-experiences structurally impossible to ship.

---

## 10. Open decisions for the lead (escalation candidates, charter §B escalation)

These are genuine forks (public-surface / semantic), surfaced with a recommended default
rather than asked blind (charter "build first; ask only at a real fork"):

1. **DiagCode namespace spelling** — recommend `MOLT-<LAYER>-<Class>` (e.g.
   `MOLT-RT-NameError`). Default: adopt this; it is stable, greppable, and doc-anchor
   friendly. (Public surface: appears in `molt explain` and docs forever.)
2. **`molt new` scope** — recommend a MINIMAL scaffold (one `.py` + a next-step line),
   deferring richer templates to the demo arc (doc 58). Default: minimal.
3. **Whether the parity region must match CPython 3.12 or 3.14 traceback formatting**
   when they differ (PEP-657 carets evolved across versions) — recommend keying the
   parity region to the `--python-version` target (3.12/3.13/3.14), since molt already
   targets per-version semantics (`cli-reference.md:46`). Default: version-keyed oracle
   rows (mirrors charter §C.3-#14 "the oracle set itself is versioned").

These three are the only decisions that encode a public-surface invariant; everything else
in this plan is implement-measure-report.
