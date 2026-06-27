<!-- 100-year world-class DX doctrine + layered architecture. Principal-architect synthesis of read-only research workflow wo98z6t05, 2026-06-27. -->

Source verification complete. Every load-bearing claim in the three reports is confirmed at the file level, with two refinements that materially improve the recommendation. Here is the foundational design document.

---

# molt DX Doctrine & Layered Architecture — The 100-Year Plan

**Status:** Foundation design doc. Release-gating product contract.
**Authority:** Principal DX architect decision packet. Recommended defaults are binding unless explicitly amended by a superseding design doc.
**Scope:** Governs every user-facing knob, default, error, and extension surface in molt.
**Grounding:** Synthesizes Report A (current-DX audit), Report B (SOTA progressive-disclosure patterns), Report C (determinism contract + subset boundary). All file references verified against the molt tree on 2026-06-27.

---

## 0. The Operator Vision, restated as an engineering invariant

> A Python that **JUST WORKS** for amateurs/hobbyists/shallow-divers — zero-config, drop-in, familiar, deterministic — that is **super powerful and performant** under the hood — with **extreme composable extensibility** for power users. Serve the **whole spectrum simultaneously**, with **NO layer leaking into another** and **NO cliff between them.**

This vision has one hard structural constraint that everything below exists to satisfy:

> **THE NO-CLIFF SUPERSET LAW.** Each layer is a *strict, additive superset* of the layer beneath it. Code, config, and mental models learned at layer N remain valid and reused at layer N+1. There is no point at which a user must discard what they know and restart in a different syntax, tool, or model. (Report B, M1–M3; Mojo's "progressive disclosure of complexity.")

Everything that follows is downstream of that law. When two designs conflict, the one that better preserves the superset law wins.

---

## 1. THE DX DOCTRINE — Six Binding Principles

These are not aspirations. They are the constitution. Every PR that touches a user-facing surface is gated against them.

### D1. Progressive disclosure is the law of the surface.
The default surface shows the 80%-case answer and nothing else. Power is *revealed on demand*, never *required up front*. **Already partially live and must be defended:** `_BuildHelpFormatter` (`src/molt/cli/arg_helpers.py:235-255`) shows only ~13 essential flags from `_BUILD_ESSENTIAL_FLAGS` (`arg_helpers.py:216-232`); the other ~40 build flags work but are hidden from `molt build --help`. This is the canonical mechanism. The doctrine elevates it from an implementation convenience to a *rule*: **no flag enters the essential set without a principal-level justification that a first-week user needs it.** (Cargo's explicit doctrine — "the more flags that exist, the more likely a user won't find the one they need" — Report B, P-cargo, M5.)

> **Binding correction (release-gating):** `--backend` is currently in the essential set (`arg_helpers.py:230`), exposing `cranelift/llvm/auto` to beginners who must not care. It is *demoted* out of the essential set. Backend selection is a Layer-1 concern, not a Layer-0 one (§2).

### D2. The no-cliff gradient is continuous and tested.
Moving from amateur to power user is a *smooth ramp of additive steps*, not a sequence of walls. The gradient is itself a tested artifact: we add CI gates that assert continuity (a Layer-0 invocation and its Layer-1/Layer-2 elaboration produce the *same semantics* unless the user explicitly changed one knob). The cliff is defined precisely (Report B): a cliff exists wherever advancing requires *restarting in a different model*. We forbid that by construction via the superset law.

### D3. Determinism is the default contract, not an opt-in.
The default build is the reproducible, CPython-parity build. **Every** knob that trades determinism for speed is opt-in, off by default, function- or build-scoped (never a silent global), and recorded in the artifact's provenance. This is already molt's posture — `--deterministic` resolves to `True` by default (`entrypoint_parser.py:276-279`, default `None` → `True` in dispatch; `fast_math.rs` fires only under an explicit `@fast_math` attribute) — and the doctrine makes it a *named, unified, profile-cross-tested guarantee* (§3). (Report C, Part 3; reproducible-builds.org definition.)

### D4. Composable abstractions, not config soup.
When users need to *branch, parameterize, or extend* behavior, the answer is a **typed, composable abstraction** — not a growing pile of flags, and not a config language that sprouts conditionals and templating. This is the explicit antidote to the Configuration Complexity Clock (Report B, A4): *when config wants to branch, that is the signal to expose a typed extension API, not to grow a config DSL.* molt's current gap is here — there is no extension algebra at all (verified: zero `register_*`/`entry_point` surface in `src/molt/cli/`); §2 Layer 2 specifies it.

### D5. One config-and-extension authority.
There is exactly **one** resolution model for "what is the effective configuration," with **one** written precedence order, **one** introspection command that shows the resolved truth *and its provenance*, and **one** extension-registration mechanism. molt is ~70% there: `_resolve_command_config` (`src/molt/cli/config_resolution.py:28-36`) is a single layered resolver and `molt config` mirrors it. The *defect the doctrine must fix*: the ~70+ `MOLT_*` env vars are a **parallel, ungoverned authority** read ad-hoc by lower layers (verified: `cargo_profiles.py` reads `MOLT_*_CARGO_PROFILE` directly from `os.environ`, entirely outside the toml/flag resolver; the 40-line footgun comment at `entrypoint_dispatch.py:247-262` exists precisely because `MOLT_STDLIB_PROFILE` has two disagreeing readers). §4 unifies this.

### D6. Honest-early subset boundary.
The boundary of what molt can compile is surfaced as a **friendly, early, deterministic compile-time diagnostic with a concrete alternative** — never a silent semantic divergence, never a cryptic backend/runtime crash, never a hidden fallback to host CPython. The machinery exists and is good (`CompatibilityIssue`/`CompatibilityReporter` in `compat.py`, carrying `feature`/`location`/`alternative`/`detail`); the doctrine's job is to route *every* boundary construct through it and kill the second `NotImplementedError` dialect (§3.4). (Report C, Part 5; Elm/Rust "compilers as assistants.")

---

## 2. THE LAYERED ARCHITECTURE

Three layers, one continuous gradient. The defining property is that **the artifacts of each layer are the literal substrate of the next** — you never rewrite, you only refine.

```
 LAYER 0  amateur        molt run foo.py                         zero config, zero flags, zero files
    │                     ───────────────────────────────────────────────────────────────────
    │   (add a flag, no rewrite — the flag just sets a default you were already getting)
    ▼
 LAYER 1  configurable    molt build foo.py --release            profiles · targets · flags
    │                     [tool.molt] / [tool.molt.<cmd>]         invisible-good defaults, made visible
    │                     [tool.molt.profile.<name>]             named composable presets
    │                     ───────────────────────────────────────────────────────────────────
    │   (a flag/profile you keep reaching for graduates into a named, inheritable profile;
    │    when you need to BRANCH or EXTEND behavior, you cross into the algebra — additively)
    ▼
 LAYER 2  power user      the EXTENSION ALGEBRA                   typed · capability-scoped · composable
                          passes · backends · types · Reprs · profiles · diagnostics
                          registered through ONE authority, discovered by typed intent
```

### Layer 0 — The amateur contract: what `molt run foo.py` MUST deliver

This is the release-gating definition of "just works." The audit's verdict is **"strong bones, weak first contact"** — the mechanism is beginner-friendly (no mandatory flags/files, `dev`/`release` only, hidden advanced flags, zero required env vars) but the literal first run violates the contract in three ways. The doctrine makes the contract explicit and binds the fixes.

**The Layer-0 contract (binding):**

1. **No config file is ever required.** ✅ Already true — `run_script` (`commands.py:601-651`) uses `_find_project_root`, not `_require_molt_root`; missing `molt.toml`/`pyproject.toml` is treated as empty config (`build_inputs.py:1067-1086`). *Defend this forever:* the presence of a config file must only ever *refine*, never *enable*, basic operation. (Report B, P2.)

2. **No flag is ever required.** ✅ Already true. All ~70 `MOLT_*` env vars have defaults; a beginner confronts zero.

3. **It must *feel* like `python foo.py`.** ⚠️ **The binding gap.** Today `molt run` is build-then-run with a *visible* multi-line build step to stderr (`commands.py:296`), and the first invocation triggers a one-time Rust runtime `cargo build` (`runtime_build.py:381`) — a multi-second-to-minutes cold start that a `python` user never sees. **Binding requirement R0.1:** the default `molt run` output on the happy path is **silent on success** except the program's own output — build progress goes to a spinner that erases itself, or behind `--verbose`. The cold runtime build emits *one* honest line ("First run: building molt runtime (one-time, ~Ns)…") and never again. The model to match is `cargo run`'s quiet success, not a compiler log.

4. **The onboarding docs must demonstrate the contract.** ⚠️ **Release-gating documentation defect.** `cli-reference.md:9` promises the clean `molt run app.py`, but `README.md:42-49` and `getting-started.md:42-49` show *only* the verbose `uv run --python 3.12 python3 -m molt.cli build examples/hello.py` form. A beginner following the README never experiences the advertised path. **Binding requirement R0.2:** the README "5-Minute Quickstart" leads with `molt run hello.py` as line one. The verbose form is demoted to a "from source / contributor" appendix. *This is the single cheapest, highest-leverage fix in the entire document.*

5. **Determinism is on by default, invisibly.** ✅ The amateur gets a reproducible, CPython-parity binary without knowing the word "deterministic" (§3).

6. **The subset boundary, when hit, teaches.** ✅ Best-in-class already for the constructs that matter: `exec`/`eval`/`compile` and non-allowlisted calls fail at compile time with `feature` + `location` + `alternative` (`call_dispatch_named.py:40-98` allowlist; `compat.py` format). The amateur who pastes `eval(user_input)` gets a friendly explanation and an alternative, not a crash.

**The one unavoidable Layer-0 honesty:** molt requires a Rust + C + `uv` toolchain up front (`getting-started.md:6-10`), which `python foo.py` does not. This cannot be hidden, but it **can** be made a *one-time, friendly, self-healing* step. ✅ `molt doctor`/`molt setup` already give OS-specific copy-pasteable remediation (`brew install`, `winget install`, `cargo install …`, `curl … | bash`) per tool (Report A confirms). **Binding requirement R0.3:** when `molt run` is invoked without a ready toolchain, it does not dump an error — it prints the single `molt setup` line that fixes it. The toolchain prerequisite becomes a 30-second guided step, not a wall.

> **Layer-0 verdict and gate:** the bones are correct and the boundary diagnostics are a genuine differentiator. Layer 0 is **release-gated on R0.1 + R0.2** (quiet success + docs that show the real path). Without those, the "just works" promise is true in the code and false in the user's hands.

### Layer 1 — The configurable contract: invisible-good defaults, made visible

Layer 1 is for the shallow-diver who wants *a little* control: "make it fast," "build for the browser," "use this Python version." The contract: **every Layer-1 knob has an invisible-good default (so Layer 0 never sees it), and reaching for the knob is a single additive step that does not invalidate anything.**

**What Layer 1 owns:**

- **Profiles.** Today: `dev`/`release` only at the user surface (`entrypoint_parser.py:780,915,1010`), with `--release` as a friendly alias. ✅ Correctly minimal.
- **Targets.** Today: default `native` everywhere; `wasm/luau/llvm/mlir` explicit opt-in (`entrypoint_parser.py:1238`). ✅ Correct.
- **Deploy profiles.** Today: `--profile cloudflare/browser/wasi/fastly` (Report A). ✅ This is *already* the "one dial sets ten dials" primitive (Report B, P4) — a profile bundles a coherent set of low-level knobs behind one intent name.
- **The layered toml surface.** Today: `[tool.molt]` and `[tool.molt.<cmd>]`, resolved by `_resolve_command_config` (`config_resolution.py:28-36`) with precedence **CLI flag > `[tool.molt.<cmd>]` > `[tool.molt]` > built-in default**, introspectable via `molt config`. ✅ This is genuinely good and SOTA-shaped (Report B, P6).
- **`--backend`** (demoted here from Layer 0 per D1).

**The three binding Layer-1 corrections:**

**R1.1 — Kill the `build`-vs-`run` default-profile divergence.** Verified defect: `build` defaults to `release` (`entrypoint_dispatch.py:136-142`, `__init__.py:888`) while `run` defaults to `dev` (`entrypoint_dispatch.py:503-511`). So `molt build app.py` and `molt run app.py` compile with *different* optimization profiles — a silent surprise that violates D2 (the no-cliff gradient must be predictable). **Decision:** unify on a single documented rule — `run` = `dev` (fast iteration) and `build` = `release` (shipping artifact) is *defensible* but must be **loudly documented at both `--help` sites and in `molt config` provenance**, OR (preferred) introduce a single `molt run --release` / `molt build --dev` symmetry so the *verb* no longer secretly implies the profile. Recommended default: **make the profile explicit in `molt config` output for both verbs**, so the divergence is never silent. (Report B, A6 — no hidden state changing behavior invisibly.)

**R1.2 — Promote the already-existing named-profile machinery to a first-class, user-definable, inheritable surface.** This is the document's most important *constructive* finding. molt **already has** a rich internal named-profile system — `dev-fast`, `release-fast`, `release-output` (`cargo_profiles.py:54,93`, with documented semantics: `dev-fast` = debug info + incremental, `release-output` = `panic=abort` + opt-level `z` for size, `release-fast` = high codegen-units + thin LTO for speed). But these are **invisible to users**, reachable only via `MOLT_*_CARGO_PROFILE` env vars (the ungoverned authority). The user-facing surface is only `choices=["dev","release"]` (verified at seven sites).

> **Decision (binding):** expose named, inheritable, user-definable build profiles in `[tool.molt.profile.<name>]`, exactly like Cargo's named custom profiles (Report B, P4 + RFC 2678). A profile declares `inherit = "release"` and overrides individual knobs. The existing `dev-fast`/`release-fast`/`release-output` become the *built-in* named profiles, documented and selectable as `--profile release-fast`. This single move:
> - collapses the long tail of advanced flags (`--split-runtime`, `--stdlib-profile micro`, `--wasm-opt-level`, `--snapshot`, `--precompile`, PGO/BOLT) into **intent-named bundles** instead of memorized flag-soup (attacks flag-explosion A5 directly);
> - migrates the `MOLT_*_CARGO_PROFILE` env vars *into* the one governed authority (advances D5);
> - is the cleanest no-cliff bridge from Layer 1 to Layer 2: a profile is the artifact you keep refining.

**R1.3 — Adopt ruff's `select`/`extend-select` two-verb model wherever there is a *set*.** Most directly for `--capabilities` and any future lint/rule surface: let users **extend** the curated default set additively (`extend-capabilities = [...]`) instead of **replacing** it (`capabilities = [...]`, which forces restating the safe defaults and invites drift). (Report B, P5.) This is the precise mechanism that keeps the additive (no-cliff) path easy and is the embodiment of the superset law at the config level.

**How a user moves Layer 0 → Layer 1 with no cliff:** they were already getting `dev` from `molt run`. They type `molt build --release` — *the same program, one additive flag, no rewrite.* They want it for the browser: `molt build --release --target wasm` — additive again. They get tired of typing it: they write four lines in `pyproject.toml` under `[tool.molt.build]` — *the flags they already know become durable*, and `molt config` shows them exactly what resolved and from where. Nothing learned was discarded. (Report B, M3 — continuity of artifacts.)

### Layer 2 — The power-user contract: THE EXTENSION ALGEBRA

This is molt's **largest current gap** and its largest 100-year opportunity. The audit verdict is unambiguous and source-verified: **power-user *operational tunability* is rich (env vars, PGO/BOLT/snapshot/split-runtime, capability manifests, type-facts feedback), but genuine *extensibility* is absent.** Backends are a hard-coded enum (`choices=["cranelift","llvm","auto"]`, verified at `entrypoint_parser.py:256,833`); passes are internal with only coarse env dials (`MOLT_MIDEND_PROFILE`); custom types/Reprs have no Python API; and a grep for *any* `register_*`/`entry_point`/`plugin` mechanism in the CLI returns **nothing**. The only pluggable boundary is the ad-hoc C-extension ABI (`molt extension`, C headers + toml manifest). **Power users can tune molt deeply; they cannot extend it without forking.**

The doctrine's answer is an **algebra**, not a flag set: a small number of typed, composable extension points that combine. The design is anchored on the SOTA gold standard (SwiftPM plugins: typed, capability-scoped, sandboxed, intent-discoverable — Report B, P7–P9) and molt's *existing* security primitive (`trust_policy.toml`/`--capabilities` — extend it, don't reinvent it).

**The five algebraic extension points** (each is a *typed value* that composes, not a string flag):

| Extension point | What it is | Composition law | Replaces today's |
|---|---|---|---|
| **Pass** | a typed IR→IR transform with declared {reads, writes, invariants-preserved} | passes compose into a **pipeline**; the scheduler orders by declared dependencies, not registration order | internal-only midend, `MOLT_MIDEND_*` dials |
| **Backend** | a typed `(TIR, Target) → Artifact` lowering with declared capabilities | backends are *selected*, and a custom backend registers a new `--backend`/`--target` value | hard-coded `cranelift/llvm/auto` enum |
| **Type / Repr** | a typed mapping from a Python type to a runtime representation + its ops | Reprs compose via the type lattice; a custom Repr declares which ops it provides and falls back to the boxed default for the rest | C-ABI only, no Python API |
| **Repr-of (display/serialization)** | a typed `value → bytes/str` for a user type, deterministic by contract | composes with the determinism contract — a custom Repr that is nondeterministic is rejected in strict builds | nothing |
| **Profile** | a named, inheritable bundle of knobs (from R1.2) — *the bridge object* | profiles inherit and override; this is the Layer-1↔2 hinge | `MOLT_*_CARGO_PROFILE` env vars |

**The four binding laws of the algebra** (these are what make it an *algebra* and not flag-soup, satisfying D4):

- **L-A (typed I/O, not strings).** Every extension declares its inputs and outputs as typed structures (à la SwiftPM's `BuildResult`/`TestResult`), never opaque strings. This is what makes them composable and cache-correct. (Report B, P7b; anti-pattern A1 — ad-hoc untyped hooks are forbidden.)
- **L-B (capabilities declared up front, default-deny).** Every extension *requests* capabilities (FS-read, FS-write, network) in its manifest; the host *grants* them; default is read-only, no network. **Reuse `trust_policy.toml`/`--capabilities` verbatim** — molt already ships this philosophy for the runtime; extend it to the compiler's extension surface. (Report B, P8; D5 — one authority.)
- **L-C (discoverable by typed intent).** Extensions declare a typed intent (`Pass`, `Backend`, `Repr`, `.formatting`, custom verb) so `molt` can answer "show me all backends / all passes" by enumeration, not metadata string-matching. (Report B, P9; SwiftPM intent enums.)
- **L-D (one registration authority).** All five points register through **one** mechanism, and that mechanism is the *same* config authority as Layer 1 (D5). Concretely: `[tool.molt.extensions]` in the toml, resolved by the *same* `_resolve_command_config` path, introspected by the *same* `molt config`.

**Two-tier extensibility, shipped in order** (Report B, P7a then P7b — they are not mutually exclusive):

1. **Tier A — ship now, near-zero cost: the `molt-*` PATH convention.** Any binary named `molt-foo` on `$PATH` is invocable as `molt foo`. Zero coupling, zero registration, zero API surface — this is Cargo's highest-ROI extensibility decision and it unblocks the *entire* community long tail (`molt-watch`, `molt-bench`, …) while the typed API is designed. This is a small, contained change to the dispatcher (`entrypoint_dispatch.py`).
2. **Tier B — the strategic in-process typed algebra above.** For first-class integrations that need structured I/O, sandboxing, and cache-correctness (custom passes/backends/Reprs). This is the multi-year investment. It is the antidote to A4: when a user wants build-time branching, they get a *typed pass*, not a config DSL.

**How a user moves Layer 1 → Layer 2 with no cliff:** the hinge is the **profile** (R1.2). A power user who has been refining `[tool.molt.profile.fast]` for months hits something a flag can't express — say, a domain-specific peephole optimization. They don't restart in a new tool: they write a `Pass` (typed, declares it preserves SSA + determinism), register it in the *same* `[tool.molt.extensions]` block they already use, and reference it from the *same* profile (`passes = ["+my_peephole"]` — note the `+`, the `extend` verb from R1.3/L-A). The toml they knew, the profile they built, the `molt config` introspection they trust — all still valid, all reused. The extension is *additive*. (Report B, M2/M3.) That continuity is the no-cliff law made concrete.

---

## 3. THE DETERMINISM CONTRACT

Determinism is a **testable contract with two halves plus a boundary clause**, unified under one name. molt already has ~80% of the mechanisms (verified: `--deterministic` default-true, opt-in-only `fast_math.rs`, `test_ir_determinism.py`, `test_entropy_audit.py`, `compat.py`); the contract's job is to *unify them under one named, profile-cross-tested guarantee* and close three concrete gaps. (Report C is the full technical spec; this section is its governing summary.)

### 3.1 What is guaranteed by default (no flag)

**Half A — Build reproducibility:** *same (source closure, target, profile, opt-in set, molt+toolchain version) → byte-identical artifact (SHA-256 equal).* Guaranteed by: compiler IR is a pure function of input (no entropy/timestamp/`PYTHONHASHSEED`/wall-clock/`id()`-ordering — gated by `test_entropy_audit.py` and `test_ir_determinism.py`, which already encode named bug classes #34 set-iteration-order-leak and #73 wall-clock-budget-leak); plus `SOURCE_DATE_EPOCH`, path remapping, codegen-unit pinning, linker-UUID normalization.

**Half B — Execution determinism:** *same binary + same input → byte-identical stdout, identical exception signature (type+message), compatible exit code — across runs, backends, AND optimization profiles.* Guaranteed by: IEEE-754 parity with **no FP contraction** by default (the subtle one — FMA fusion can be on at `-O2` even without `-ffast-math`; must be explicitly `-ffp-contract=off`); dict insertion order preserved (a *language guarantee* since 3.7, not optional); finalization order defined by the **ownership lattice** (CLAUDE.md Council Doctrine), invariant across profiles/backends.

**The CPython-parity calibration (binding):** by default molt is *exactly as deterministic as CPython with the same hash seed* — runtime hash seed is random by default (faithful to `PYTHONHASHSEED` unset), so a program whose output depends on set-of-strings iteration order is nondeterministic under molt *exactly as it is under CPython*. molt must never be *less* deterministic than CPython-same-seed, and `--deterministic` makes it *more* (pins seed to 0). (Report C, 1.3–1.4; `ops_hash.rs` reproduces CPython's SipHash key schedule bit-for-bit.)

### 3.2 Which fast-but-nondeterministic behaviors are opt-in (off by default)

The governing rule (D3): **there is no single flag that turns determinism off globally.** Every relaxation is function- or build-scoped and provenance-recorded.

| Capability | Default | Opt-in surface | Effect when ON |
|---|---|---|---|
| FP fast-math (reassoc/FMA/reciprocal/finite-only) | OFF | `@fast_math` decorator — **function-scoped only**, no global `--fast-math` | output may diverge from CPython and across CPU/backends; **excluded** from parity + reproducible-output gates |
| FP contraction (FMA fusion) | OFF | folded into `@fast_math`, never standalone | last-bit divergence from CPython |
| Runtime hash seed | **random (CPython parity)** | `--deterministic` pins to 0; or honor `PYTHONHASHSEED` | deterministic mode makes set/str-dict order stable run-to-run |
| Non-deterministic codegen (high codegen-units) | allowed in **dev** profile only | release-output/`--deterministic` force reproducible config | dev binaries labeled `determinism: relaxed` |

**Provenance (binding):** every binary embeds a determinism manifest — which opt-ins were active, target version, profile, toolchain hash. A clean build is labeled `determinism: strict`; any `@fast_math` function or dev-profile build is `determinism: relaxed` with reasons. This lets CI and `molt config` answer "is this in the reproducible+parity class?" deterministically.

### 3.3 `--deterministic` is the explicit *strengthening* (already exists; formalized)

`--deterministic` (default-on, `entrypoint_parser.py:276`) means precisely: pin runtime hash seed to 0; **forbid `@fast_math`** (compile error unless `--allow-fast-math` acknowledges the downgrade); select reproducible codegen/link config; set `SOURCE_DATE_EPOCH` + path remapping; assert the manifest is `strict`.

### 3.4 The subset boundary as friendly, early, deterministic compile errors

The boundary (`STATUS.md:18-19`): no unrestricted `exec`/`eval`/`compile`, no runtime monkeypatching, no unrestricted reflection. The DX contract (D6):

- **Route every boundary construct through the one reporter** (`compat.py` `CompatibilityReporter.unsupported`), raised in the **frontend AST pass** (early), with `feature` + `file:line:col` + a **mandatory `alternative`**.
- **The boundary is precise, not blunt.** *Statically-resolvable* forms are *supported*: `getattr`/`setattr`/`globals`/`locals` in resolvable forms already emit guarded fast paths (`call_dispatch_named.py:369,487`); `ast.literal_eval`/`eval`-of-literal can be supported. Only the *dynamic/unrestricted* form errors, and the diagnostic must say *which* and suggest a dict-dispatch / `match` / Protocol-typed alternative. This converts the boundary from a frustration into a teaching moment.
- **The errors are deterministic** — same program → same set of compat errors in the same order on every seed/host. This is *coupled to* and *gated by* the determinism harness: `test_ir_determinism.py` already compares the `COMPILE_ERROR::<type>::<message>` outcome across seeds. A compat error whose text/order varies with the hash seed is a determinism bug, already caught.

**Binding correction (release-gating): kill the second error dialect.** A long tail of constructs still raise bare `NotImplementedError("... unsupported")` (verified pattern across `call_dispatch_builtin_scalar.py`, `expressions.py`, `call_dispatch_named.py`) — no location, no alternative, reads like an internal assert leaking to users. **Every user-reachable `NotImplementedError` on the lowering path is migrated to a `CompatibilityIssue`** with location + alternative. Diagnostic quality must be *uniform*, not "excellent for a curated set, bare for the tail."

### 3.5 The three concrete gaps the contract must close (release-gating)

1. **Reproducible *linked binary*, not just `.o`.** Today repro gates lean on `--object` to dodge linker-injected nondeterminism (macOS `LC_UUID`). Promote the fix from "compare object files" to "the linked binary itself is byte-identical" via UUID/build-id normalization + path remapping. (`tools/check_reproducible_build.py`.)
2. **Profile-cross gates.** Add CI gates asserting (a) same stdout *and* same finalizer-order trace across `dev`/`release-fast`/`release-output`, and (b) FP-contraction-off in default codegen. Catches the RC-timing-leak hazard (`20_rc-ownership-drop-insertion.md`) and the FMA hazard.
3. **`@fast_math` exclusion + strict-blocking.** A `@fast_math` function is *excluded* from the parity + reproducible-output lanes (not silently failing them), and is a compile error in a `strict`/`--deterministic` build unless `--allow-fast-math` is passed.

---

## 4. HOW THE DOCTRINE GOVERNS EVERY KNOB

The test of a doctrine is that it *decides* concrete knobs without further debate. Applied to the four families the operator named, all unified under the **one config/extension authority (D5)**:

### 4.1 Concurrency model — GIL-compat | free-threaded | subinterpreter

This is the textbook case for the doctrine. The three modes map cleanly onto the three layers:

- **Layer 0 (amateur):** there is an **invisible-good default**, chosen by molt, never surfaced. **Recommended default: GIL-compatible semantics** — it is the most CPython-faithful (D3 — determinism/parity as default) and the least surprising for drop-in code. The amateur never types the word "GIL," "free-threaded," or "subinterpreter." (D1.)
- **Layer 1 (configurable):** the mode is a single knob with an invisible-good default — `--concurrency {gil,free-threaded,subinterpreter}` and `[tool.molt.concurrency]`, resolved through the one authority with `molt config` provenance. The shallow-diver who *knows* they want free-threading flips one additive flag; nothing else changes. (D2, D5.)
- **Layer 2 (power user):** fully available and **composable with the determinism contract** — free-threaded is, by construction, a relaxation of single-threaded determinism, so a free-threaded build is labeled in the provenance manifest exactly like `@fast_math` is (§3.2), and the determinism guarantees scope to "no *additional* nondeterminism beyond what the source's concurrency implies" (Report C, 1.7). The mode is also a legal field in a *named profile* (R1.2), so `[tool.molt.profile.server]` can bundle `concurrency = "free-threaded"` with its other knobs.

**Decision:** GIL-compat is the binding default. Free-threaded and subinterpreter are first-class, fully-available Layer-1 flags and Layer-2 profile fields, never Layer-0 surface, always provenance-recorded when they relax determinism.

### 4.2 Build profiles

Governed entirely by R1.1 (kill the silent `build`/`run` divergence) + R1.2 (promote `dev-fast`/`release-fast`/`release-output` to user-visible, inheritable, composable named profiles). The doctrine's decision: **the profile is the primary unit of Layer-1 power and the Layer-1↔2 hinge.** Flag-soup is forbidden (A5); a coherent bundle of knobs always gets a name.

### 4.3 Targets

`native` (default) / `wasm` / `luau` / `llvm`. Already correctly layered (Layer 0 never sees a target; Layer 1 opts in with one flag). The doctrine's extension: a Layer-2 **custom backend** (the algebra, §2) can *register a new target value*, so the fixed enum (`entrypoint_parser.py:1238`) becomes an *extensible registry* — the same way `molt-*` PATH commands extend verbs. The aspirational endpoint (Report B, P18): a single **universal/cross-target lockfile** (`molt.lock`) that resolves per-target, à la uv — unusually apt for a native×wasm×luau × Python-3.12–3.14 compiler.

### 4.4 stdlib-profile (`full` | `micro`)

Today a real footgun: defaults are scattered (`__init__.py:902` `micro`, `commands.py:1184,1267` `micro`, dispatch fallback `entrypoint_dispatch.py:243-287`), and the 40-line comment at `entrypoint_dispatch.py:247-262` documents that env-only `MOLT_STDLIB_PROFILE=full` can desync from a `micro` staticlib and cause **link failures**. This is a direct violation of D5 (one authority) — the env var is a second reader that disagrees with the flag/default reader.

**Decision (binding):** `stdlib-profile` is resolved **only** through the one config authority (flag → `[tool.molt.<cmd>]` → `[tool.molt]` → single default), and `MOLT_STDLIB_PROFILE` becomes (at most) a documented alias that flows *into* that resolver — never a second independent reader. The single default is consolidated to one constant. `molt config` reports the resolved value and its source. This is the template for migrating *all* ~70 `MOLT_*` env vars off the parallel authority and under D5.

---

## 5. CURRENT-DX VERDICT

**Is molt's DX today conducive to the operator vision — amateur "just works" + power-user composability? Verdict: the foundations are unusually strong and the doctrine is *recoverable without re-architecture*, but it is NOT yet conducive on two release-gating axes.**

**(a) Amateur "just works" — PARTIAL: strong bones, weak first contact.**
- *Conducive:* no mandatory flags or config files (`commands.py:601-651`, `build_inputs.py:1067-1086`); `dev`/`release` only; ~40 advanced flags hidden from `--help` (`arg_helpers.py:235-255`); zero required env vars; friendly OS-specific toolchain remediation (`doctor`/`setup`); **best-in-class early structured errors** for the constructs that matter (`compat.py` + allowlist at `call_dispatch_named.py:40-98`).
- *Not conducive (release-gating):* (1) `molt run` is a *visible* build with a one-time Rust `cargo build` cold start, not quiet like `python foo.py` (R0.1); (2) the primary onboarding docs (`README.md:42-49`, `getting-started.md:42-49`) showcase the verbose `uv run python3 -m molt.cli …` form and **never** the clean `molt run` that `cli-reference.md:9` promises (R0.2). Secondary: `build`/`run` default-profile divergence (R1.1); long-tail bare `NotImplementedError` (§3.4).

**(b) Power-user composable extensibility — ABSENT for the compiler.**
- *Conducive:* rich *operational* tunability (env vars, PGO/BOLT/snapshot/split-runtime, capability manifests, type-facts feedback); **one coherent toml+flag config surface** with a single resolver (`config_resolution.py:28-36`) and `molt config` introspection — genuinely SOTA-shaped.
- *Not conducive (the strategic gap):* **no typed, composable API** to add a backend (hard-coded enum, verified `entrypoint_parser.py:256,833`), a pass (internal, coarse env dials only), a type, or a Repr. **Zero** `register_*`/`entry_point`/`plugin` surface in the CLI (verified by grep). The only external boundary is the ad-hoc C-ABI. **Power users can tune molt deeply; they cannot extend it without forking.** This is the single largest deviation from the operator vision.

**(c) Determinism — STRONG, among the best commitments in the tree.** Default-on (`entrypoint_parser.py:276`), enforced via lockfiles + pinned hash seed (`ops_hash.rs` CPython-faithful) + opt-in-only fast-math (`fast_math.rs`) + IR-determinism gates with *named bug classes* (`test_ir_determinism.py` #34/#73) + a static entropy audit (`test_entropy_audit.py`). This axis is already conducive; it needs *unification under one name* and three gap-closures (§3.5), not invention.

**(d) Config authority — MOSTLY one surface, with a documented parallel authority.** The toml+flag resolver is coherent and introspectable. The defect: ~70 `MOLT_*` env vars form a second, ungoverned authority that the code *itself* documents as drift-prone (`entrypoint_dispatch.py:247-262`, verified `cargo_profiles.py` reads env directly). D5/§4.4 fix this.

**Overall:** molt has done the *hard* things right (determinism, a single config resolver, hidden-flag progressive disclosure, structured boundary diagnostics) and left the *cheap-but-high-leverage* things (quiet first run, honest docs) and the *strategic* thing (the extension algebra) undone. The doctrine is achievable as **refactor + addition**, not rewrite.

---

## 6. THE PHASED 100-YEAR DX ROADMAP

Ordered by leverage (impact × fit). Each phase names the molt files to refactor. Phases 0–1 are release-gating for the "just works" contract; Phase 3 is the strategic 100-year investment.

### Phase 0 — Honest first contact (release-gating, days, near-zero risk)
The cheapest, highest-leverage fixes. Refactor:
- **`README.md`, `docs/getting-started.md`** — lead the quickstart with `molt run hello.py`; demote `uv run … -m molt.cli build …` to a contributor appendix. **(R0.2.)**
- **`src/molt/cli/commands.py`** (`_run_wrapper_build` ~line 700, build diagnostics ~296) — make happy-path `molt run` quiet on success; spinner that erases; one honest line for the one-time runtime build. **(R0.1.)**
- **`src/molt/cli/runtime_build.py`** (`_ensure_runtime_lib`, lines 381/1339) — the cold-build message is the *single* "First run…" line, never repeated.
- **`src/molt/cli/arg_helpers.py:216-232`** (`_BUILD_ESSENTIAL_FLAGS`) — demote `--backend` out of the essential set. **(D1.)**

### Phase 1 — Coherence & honest errors (release-gating, weeks)
Eliminate the silent surprises and the second error dialect. Refactor:
- **`src/molt/cli/entrypoint_dispatch.py:136-142, 503-511`** — resolve the `build`/`run` default-profile divergence; surface the resolved profile in `molt config`. **(R1.1.)**
- **`src/molt/frontend/visitors/*.py`** (`call_dispatch_builtin_scalar.py`, `expressions.py`, `call_dispatch_named.py`, et al.) — migrate every user-reachable `NotImplementedError` to `compat.py`'s `CompatibilityReporter.unsupported(...)` with `location` + mandatory `alternative`. **(D6, §3.4.)**
- **`src/molt/compat.py`** — adopt the Elm/Rust diagnostic schema in full: add a **machine-applicability tier** (`MachineApplicable`/`MaybeIncorrect`/…) to `CompatibilityIssue`, and a source-span caret render. Reuse the Rust ecosystem you're already in (**miette**/ariadne) for TTY rendering; unify `--json`/`--diagnostics-file` on this one structured schema. Write a **diagnostics style guide** into `CONTRIBUTING.md`. **(Report B, P10–P14.)**

### Phase 2 — One authority, named profiles (months)
Unify config and promote the profile primitive. Refactor:
- **`src/molt/cli/cargo_profiles.py`** + **`src/molt/cli/entrypoint_parser.py`** (the seven `choices=["dev","release"]` sites) + **`src/molt/cli/config_resolution.py`** — expose `dev-fast`/`release-fast`/`release-output` as user-visible built-in named profiles; add `[tool.molt.profile.<name>]` with `inherit` + per-knob override; migrate `MOLT_*_CARGO_PROFILE` to flow *into* the one resolver. **(R1.2.)**
- **`src/molt/cli/entrypoint_dispatch.py:243-287, 247-262`** + **`__init__.py:902`** + **`commands.py:1184,1267`** — consolidate `stdlib_profile` to a single default and a single resolver reader; make `MOLT_STDLIB_PROFILE` an alias into it, not a second authority. **(§4.4, D5.)**
- **`src/molt/cli/config_resolution.py`** + `molt config` (`entrypoint_dispatch.py:957`) — `molt config --json` reports each value's **provenance** (default vs file vs env vs flag). **(D5, A6.)**
- **`src/molt/cli/capability_spec.py`** / `config_resolution.py:43-48` — add ruff-style `extend-capabilities`. **(R1.3.)**

### Phase 3 — The extension algebra (the 100-year investment, quarters→years)
The strategic gap. Two tiers, shipped in order. Refactor/add:
- **Tier A (now):** **`src/molt/cli/entrypoint_dispatch.py`** — the `molt-*` PATH subcommand convention. Small, contained, unblocks the community long tail. **(Report B, P7a.)**
- **Tier B (the algebra):** a new `src/molt/ext/` package defining the five typed, capability-scoped, intent-discoverable extension points (Pass, Backend, Type/Repr, Repr-of, Profile) registered through `[tool.molt.extensions]` via the *same* config authority. Refactor **`src/molt/cli/entrypoint_parser.py:256,833`** (backend) and **`:1238`** (target) from fixed enums into **extensible registries**. Reuse **`src/molt/cli/capability_spec.py`** / `trust_policy.toml` for the default-deny capability model (L-B). Model on SwiftPM (typed I/O, sandbox, intent enums) and the existing wrapper-cache identity for build-tool-plugin staleness. **(D4, §2 Layer 2, Report B, P7b–P9.)**

### Phase 4 — Determinism unification & cross-target lock (ongoing)
Promote determinism from "ten cooperating mechanisms" to "one named, profile-cross-tested contract." Refactor:
- **`tools/check_reproducible_build.py`** — reproducible *linked binary* (UUID/build-id normalization + path remapping), demoting `--object` to diagnostic-only. **(§3.5 gap 1.)**
- **`tests/determinism/`** (new gate) + **`runtime/molt-passes/src/tir/passes/fast_math.rs`** + the float-codegen pass — profile-cross stdout + finalizer-order gate; FP-contraction-off rule; `@fast_math` exclusion-from-parity + strict-build-blocking. **(§3.5 gaps 2–3.)**
- **`src/molt/cli/lockfiles.py`** — toward a universal cross-target `molt.lock` (intent in `pyproject.toml`, exact in the lock) that also pins the *molt compiler/runtime version* that built the artifact. **(Report B, P15–P18.)**

### The 100-year frame, one line
molt's north star is **Mojo's "progressive disclosure of complexity" applied to tooling, not just language**: `molt run app.py` just works and stays quiet (Layer 0); named profiles and `[tool.molt]` disclose power as one-dial-sets-many (Layer 1); a `molt-*` + typed-algebra pair discloses extensibility without an API cliff (Layer 2); Elm/Rust-grade diagnostics *on the Python source* are the handrail that teaches every next layer; and a lock + pinned-toolchain + determinism-by-default + fail-closed mode make all of it reproducible — **each tier a strict, additive superset of the one below, so there is never a point where the user must throw away what they know and start over.** The bones are already right. The work is to make first contact honest (Phase 0–1), make the one authority truly one (Phase 2), and build the extension algebra the vision requires (Phase 3).