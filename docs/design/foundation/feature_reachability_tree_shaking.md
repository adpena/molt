# Reachability-Driven Runtime-Feature Elimination

Status: DESIGN (ready_to_implement after Phase 0 lands)
Owner: feature-gate / tree-shaking authority
Scope: the gratuitous-heavy-import bug class, and its permanent structural cure.
Companion facts (verified in-tree 2026-06-26):
- Frontend gate: `src/molt/cli/module_stdlib_policy.py:130` (`_enforce_profile_feature_availability`)
- Symbol→feature authority: `src/molt/_runtime_feature_gates.py:36` (`RUNTIME_FEATURE_GATES`), `:214` (`LINK_AFFECTING_FEATURES`)
- Drifted profile model: `src/molt/cli/runtime_features.py:104` (`_ALL_DOMAIN_FEATURES`), `:153` (`_runtime_builtin_features_for_profile`)
- The reachability fact (already exists, wrong layer): `runtime/molt-tir/src/passes/intrinsics_manifest.rs:61` (`compute_intrinsic_manifest`)
- Symbol-set authority + fail-closed: `runtime/molt-tir/src/intrinsic_symbols.rs:50` (`runtime_intrinsic_symbols_required`)
- Native dead-strip resolver: `runtime/molt-backend/src/native_backend/simple_backend/app_resolver.rs:35`; runtime side `runtime/molt-runtime/src/intrinsics/registry.rs:55` (`molt_set_app_intrinsic_resolver`)
- Cargo feature ladder: `runtime/molt-runtime/Cargo.toml:15-125`

---

## 0. One-paragraph thesis

molt **already computes a whole-program, data-flow-complete intrinsic-reachability
closure** — `compute_intrinsic_manifest` in `molt-tir` — and the native backend
already emits a per-app resolver covering exactly that set so the linker
dead-strips every unreached intrinsic. But molt makes the *feature-requirement
decision* (which Cargo features the runtime archive must build, and whether to
refuse the build) from a **second, coarser, drift-prone fact**: the per-module
presence of an intrinsic literal anywhere in the static import graph
(`module_required_intrinsic_names`). The two facts disagree: importing `re`
forces `stdlib_regex` under the frontend gate even when no regex call is ever
reached, and — because of an unrelated drift bug — `stdlib_regex` is missing
from the Python profile model entirely, so `import re` is refused on *every*
profile including `full`. The canonical fix is to **delete the coarse fact as a
requirement source and make the existing TIR-reachability closure the single
authority** for (a) the required link-affecting feature set, (b) the
compile-time refusal, and (c) the per-app resolver / dead-strip — with a
curated, fail-closed implicit/dynamic-edge table for the edges static analysis
cannot see (the GraalVM reachability-metadata / Nuitka implicit-imports model).
After this, an unreached `molt_re_*` section is *unexpressible* in the binary,
an unprovable dynamic edge is *refused or retained* (never silently dropped),
and the import-vs-feature drift class is *gone* because there is only one fact.

---

## 1. The bug class, mechanically

### 1.1 What "gratuitous heavy import" means

A program writes `import re` (often transitively — `re` sits under
`warnings` → `importlib.abc` → `importlib.metadata`) but never executes a regex
operation. Under today's gate the mere *presence* of `re/__init__.py` in the
static import graph forces the `stdlib_regex` Cargo feature, which:

1. is refused at compile time on any profile whose Python model lacks
   `stdlib_regex` (see §1.3 — currently *all* of them), and
2. when not refused, links the entire `molt-runtime-regex` crate into the
   archive even though the program reaches zero of its symbols.

The same shape recurs for `csv`, `struct`/`datetime`/`base64` (→ `stdlib_serial`),
`email`, `unicodedata`/`html` (→ `stdlib_text`), `zipfile` (→ `stdlib_archive`),
`decimal`, `ast`. The FIND catalog enumerated the instances; this design cures
the *class*.

### 1.2 The two facts and where they live

```
                         IMPORT GRAPH (static, module-level only for non-entry)
                                    │
        ┌───────────────────────────┴───────────────────────────────┐
        │ FACT A (coarse, presence-based)                            │ FACT B (precise, reachability-based)
        │ module_required_intrinsic_names(path)                      │ compute_intrinsic_manifest(ir.functions, symbols)
        │  = ast.walk(module) for require_intrinsic("molt_*")        │  = every const_str op across ALL compiled TIR
        │    literals — whole-file, no call-graph, no liveness       │    functions whose value ∈ linked staticlib symbols
        │ src/molt/stdlib_intrinsic_policy.py:97                     │ runtime/molt-tir/src/passes/intrinsics_manifest.rs:61
        └───────────────┬────────────────────────────────────────────┴───────────────┬───────────────┘
                        │                                                              │
            DRIVES (today)                                                  DRIVES (today)
        ┌───────────────▼───────────────┐                          ┌───────────────────▼───────────────────┐
        │ _enforce_profile_feature_      │                          │ emit_app_resolver_function(manifest)   │
        │   availability  → REFUSE/PASS  │                          │  → per-app resolver; linker dead-strips │
        │ (module_stdlib_policy.py:130)  │                          │   every unreached intrinsic            │
        └───────────────┬───────────────┘                          └───────────────────┬───────────────────┘
                        │                                                              │
            chooses link-affecting feature requirement              chooses which intrinsics survive in the binary
            (compared against runtime_features profile model)        (precise, correct, data-flow-complete)
```

**The defect is structural, not a typo:** the requirement decision and the
elimination decision are computed by two different passes, at two different
pipeline stages, with two different precisions, and the *coarse, earlier* one is
authoritative for the decision that blocks builds. Fact B is strictly more
precise than Fact A and is already trusted to drive the binary contents; it is
simply not consulted for the feature-requirement / refusal decision.

### 1.3 The drift bug that makes it bite on every profile (the CRUX)

`runtime_features._ALL_DOMAIN_FEATURES` (`runtime_features.py:104`) is the Python
model of "what features does profile P provide." Verified 2026-06-26: it
contains `stdlib_csv` and `stdlib_math` but is **missing** `stdlib_regex`,
`stdlib_itertools`, `stdlib_path`, `stdlib_difflib`, `stdlib_xml`,
`stdlib_ipaddress`, `simdutf`, and `stdlib_crypto_legacy` — all of which the
real `stdlib_full` Cargo chain links (`Cargo.toml:20-31`, `:51-56`).

`_runtime_builtin_features_for_profile("full", …)` returns
`_ALL_BUILTIN_FEATURES + _ALL_DOMAIN_FEATURES + _MICRO_BASE_RUNTIME_FEATURES`
(`runtime_features.py:159`), which therefore *never* contains `stdlib_regex`.
So `_profile_feature_gap_for_module(re/__init__.py, full_features)` is non-empty
→ the gate refuses → the user is told "Rebuild with `--stdlib-profile full`",
which **cannot help** because the Python "full" model does not know full links
`stdlib_regex`. The refusal message is actively misleading.

This drift went uncaught because `tests/cli/test_cli_profile_feature_refusal.py`
tests `stdlib_ast`/`stdlib_crypto`/`sqlite`/`stdlib_text`/`stdlib_stringprep`/
`stdlib_zoneinfo` — but has **no test importing `re`/`itertools`/`difflib`/`xml`/
`ipaddress` on the full profile**. The reachability redesign makes this entire
*class* of "Python model drifts from Cargo ladder" drift unexpressible, because
the required-feature set is derived from the symbols actually reached and
validated against the symbols the staticlib actually defines — never from a
hand-maintained Python mirror of the Cargo chain.

### 1.4 Why the unit of elimination is the *intrinsic*, not the call, and not (only) the module

`re/__init__.py:9-24` and `decimal.py:13` call `require_intrinsic("molt_re_*"/
"molt_decimal_*")` as **top-level module-body statements**. `require_intrinsic`
(`_intrinsics.py:115`) resolves the symbol eagerly when the module body runs at
import. Two consequences fix the design space:

- You **cannot** keep `import re` in the binary image while dropping the regex
  symbols *if the module body is executed*: importing `re` runs its body and
  demands the symbols at load.
- Therefore reachability must be computed at the granularity of **the intrinsic
  symbol reached by executed code**, which is exactly what Fact B already does:
  it scans the *compiled TIR* (post-dead-code, post-inline, post the frontend's
  own elimination of unexecuted top-level statements) for the const-string
  intrinsic names that survive into codegen. A module that is imported-but-never-
  load-executed, or whose regex-touching code is dead, contributes no `const_str`
  to the TIR and thus no requirement. This is strictly the GraalVM closed-world
  model specialized to intrinsic granularity, and it subsumes both "module-level
  dead-import elimination" and "function-level dead-call elimination" because
  both manifest identically as "the `molt_re_*` const_str is absent from the
  reached TIR."

> Note the subtle correctness point this preserves: if `re`'s module body *is*
> executed (the program genuinely imports and runs `re`), its top-level
> `require_intrinsic("molt_re_compile")` *will* appear as a reached `const_str`,
> so the feature is required. Reachability does not under-approximate a genuinely
> loaded module; it only stops *unloaded / dead* modules from forcing features.

---

## 2. Design goals and non-goals

Goals:
- G1. One canonical reachability fact drives feature requirement, refusal, and
  binary elimination. Delete the per-module presence requirement.
- G2. `import re` with no reached regex call MUST NOT require `stdlib_regex`.
- G3. Fail closed on every edge static analysis cannot prove (dynamic dispatch
  to an intrinsic, `__getattr__`-reached intrinsic, reflective import that could
  load a gated module). Never silently drop; never emit an undefined-symbol link
  error; never produce a runtime "intrinsic unavailable".
- G4. Kill the import↔feature drift class (§1.3) permanently: required features
  are derived + validated against the linked staticlib, not a hand-maintained
  Python mirror.
- G5. No per-module special cases, no allowlist papering, one mechanism.
- G6. Quantified wins: binary size, cold start, and micro-buildability of
  programs that gratuitously import heavy modules.

Non-goals:
- Not changing the runtime intrinsic ABI or the per-app resolver data layout
  (`app_resolver.rs` stays; it gains an authoritative upstream).
- Not redesigning the Cargo feature *groups* (the sections are correct; their
  *selection* is what changes).
- Not eliminating profiles for ergonomics/CI fixed-archive caching — profiles
  are demoted to a coarse *upper bound + cache key*, not a requirement source
  (§5).

---

## 3. The new fact: `RequiredFeatureSet`, derived from reachability

### 3.1 Definition

For a given build (entry module, target, profile), define:

```
ReachedIntrinsics(build)  := compute_intrinsic_manifest(all_compiled_TIR_functions,
                                                         staticlib_symbol_set)
                             ∪ ImplicitClosure(ReachedIntrinsics)        # §4
RequiredLinkFeatures(build) := { link_affecting_feature_gate_for_symbol(s)
                                 for s in ReachedIntrinsics(build) } \ {None}
```

`RequiredLinkFeatures` is the **minimal set of link-affecting Cargo features the
runtime archive must build** so that every reached intrinsic resolves. It
replaces the per-module gap computation as the requirement authority.

This reuses, unchanged:
- `compute_intrinsic_manifest` (the reachability closure) —
  `intrinsics_manifest.rs:61`.
- `link_affecting_feature_gate_for_symbol` (symbol→link-affecting-feature) —
  `_runtime_feature_gates.py:242`. This is already the exact "does dropping this
  feature remove the symbol definition" predicate, so it is the correct
  section/no-section classifier. Resolver-only features (`stdlib_logging` etc.)
  correctly contribute nothing (their symbols are always defined), exactly as
  today.

### 3.2 The chicken-and-egg, and its resolution

`compute_intrinsic_manifest` needs the compiled TIR (Fact B is computed during
codegen). Feature selection chooses the runtime archive to build/link. The
symbol-set validation (`runtime_intrinsic_symbols_required`,
`intrinsic_symbols.rs:50`) needs a *linked staticlib* to extract `molt_*`
symbols from. Three quantities, mutually entangled. Resolve it with a **two-pass
build over a single full-superset archive cache**, which is how molt already
stages symbols (`MOLT_RUNTIME_INTRINSIC_SYMBOLS`):

1. **Reachability pass (frontend → TIR, no link).** Compile the program to TIR
   (this already happens). Compute `ReachedIntrinsics` against the
   **full-profile** staticlib symbol set (the maximal set; any reached intrinsic
   is a member). This is a pure analysis over IR + a cached symbol file — no
   per-program runtime rebuild. Derive `RequiredLinkFeatures`.
2. **Selection.** The runtime archive to link is the **smallest cached archive
   whose feature set ⊇ `RequiredLinkFeatures`** (§5.2). Because archives are
   built once per feature-set and cached (existing behavior keyed by
   `runtime_fingerprints`), this is a cache lookup, not a per-program compile.
3. **Codegen + link pass.** Emit the per-app resolver from the *same*
   `ReachedIntrinsics` (Fact B), validated against the *selected* archive's
   symbol set. Link. The linker dead-strips every unreached intrinsic within the
   selected features.

The symbol-set precondition is satisfied because the full-profile symbol file is
extracted and cached once (it is a superset and is profile-stable per toolchain);
reachability membership-tests against it are exact for any program. The
fail-closed contract in `intrinsic_symbols.rs:50` is preserved: if the symbol
file is unavailable, the build fails closed rather than guessing.

> This is the same shape as PyOxidizer's observe-then-filter and GraalVM's
> tracing-agent → metadata loop, but **static**: instead of running the program
> to observe loads, molt reads the reached intrinsics straight out of the TIR it
> already compiled. No execution, no guessing.

### 3.3 Where it plugs in (exact seam)

Today: `module_graph.py:780` calls `_enforce_profile_feature_availability(...)`
*before* codegen, using Fact A, and `runtime_build.py:278` selects features via
`_runtime_builtin_features_for_profile(profile, …)` using the profile model.

New: a single `required_features.py` authority computes `RequiredLinkFeatures`
from the reachability pass, and **both** the refusal and the archive selection
consume it:
- `runtime_build.py` selects the archive from `RequiredLinkFeatures` (§5.2),
  not from a flat profile expansion.
- the refusal (now a *thin* check, §3.4) compares `RequiredLinkFeatures` to the
  selected archive's feature set.

---

## 4. The dynamic-edge contract (the load-bearing soundness piece)

`compute_intrinsic_manifest` is already remarkably complete: its docstring
(`intrinsics_manifest.rs:61-103`) documents that it captures intrinsic names
reaching `require_intrinsic` *through arbitrary data flow* — direct calls,
wrapper calls (`_require_callable_intrinsic("molt_gc_collect")`), and
object-field stashing (`_LazyIntrinsic("molt_sys_version_info")`). Because it
keys on **every `const_str` whose value is a real intrinsic symbol**, anywhere
in the reached TIR, it is data-flow-complete for *statically-constant* intrinsic
names. The soundness frontier is therefore narrow and precisely characterizable:

### 4.1 The only unsound edge: a non-constant intrinsic name

If an intrinsic name is *computed* (not a `const_str`) — e.g. assembled from
runtime data and passed to `require_intrinsic` — Fact B cannot see it. This is
the molt analog of GraalVM's unregistered reflection and JS's computed dynamic
`import()`. There are two sub-cases, both handled fail-closed:

- **Within molt's own stdlib/runtime:** intrinsic names are always string
  literals by construction (the policy in `stdlib_intrinsic_policy.py` only
  recognizes literal-first-arg `require_intrinsic` calls; a computed name is
  *already* unsupported and would already fail at runtime today). So this
  sub-case is empty for first-party code by an existing invariant. Add a
  **TIR-level gate** (§7, `nonconst_intrinsic_name_is_refused`) that detects a
  `require_intrinsic`/`load_intrinsic` call whose name argument is not a
  reached `const_str` and **refuses the build** with an actionable message,
  rather than silently emitting a resolver that returns 0 → runtime
  "intrinsic unavailable". This converts the GraalVM `MissingReflectionRegistrationError`
  (a runtime `Error` that must not be caught) into a *compile-time* refusal —
  strictly better, matching molt's "no silent divergences" doctrine.

- **User code reaching an intrinsic dynamically:** user programs do not call
  `require_intrinsic` (it is a stdlib/runtime-internal boundary); they reach
  intrinsics only through stdlib surfaces, whose names are first-party literals.
  So the same gate covers it.

### 4.2 The implicit-edge table (Nuitka `getImplicitImports` / GraalVM metadata analog)

Some structural facts pull intrinsics that **no `const_str` in the reaching
module shows**, because the dependency is encoded in the runtime, not the Python
source. The canonical example is already special-cased in prose but not in one
table: **`asyncio` imports `ssl` eagerly even on micro**, and SSL keeps a
deliberately always-linkable ABI (`_runtime_feature_gates.py:121-123`). Other
candidates: a runtime-bootstrap intrinsic that the codegen emits implicitly; a
domain whose intrinsic A always transitively needs intrinsic B at runtime.

Make these **one generated `IMPLICIT_INTRINSIC_EDGES` table**, keyed exactly like
`RUNTIME_FEATURE_GATES`, of the form `reached_intrinsic_or_module → forced_intrinsics`.
`ImplicitClosure` (§3.1) is its transitive closure over `ReachedIntrinsics`.
This is the single home for every "static analysis cannot see this edge" fact —
no scattered special cases (G5). Today there is effectively one such fact (SSL's
always-linkable ABI sidesteps it by being ungated); the table formalizes the
category so the *next* one lands as a data row, not a code special-case.

Rule: **adding a row is the only sanctioned way to encode an unanalyzable edge.**
A reviewer who sees a new `if module == "...":` special-case in the gate rejects
it and asks for a table row, exactly as Nuitka funnels hidden imports into
`ImplicitImports.py` and GraalVM funnels them into `reachability-metadata.json`.

### 4.3 Fail-closed default

If the reachability pass cannot prove the graph is fully analyzable (a computed
intrinsic name per §4.1, a syntax error that prevents TIR production for a
reachable module, or a reflective import that could load a *gated* module whose
membership cannot be decided), the build **refuses** with the actionable
profile message — it does not fall through to "link everything" (fails open) nor
"drop and hope" (silently wrong). This mirrors the existing fail-closed posture
of `runtime_intrinsic_symbols_required` and the JS "retain the whole dynamic
candidate set, never drop on a guess" rule. For the *reflective-import-could-
load-a-gated-module* case specifically, the conservative-correct action is to
fall back to the **profile upper bound** (§5) for that build — i.e. the existing
`runtime_import_support` fan-out already widens the graph; the required-feature
set for such builds is the profile's full link-affecting set, not the empty set.

---

## 5. The role of profiles after this change

Profiles are **not retired** — they are **demoted from a requirement source to a
coarse upper bound + archive-cache key**. Concretely:

### 5.1 Profile = upper bound (a ceiling, never a floor)

A profile `P` defines the *maximum* link-affecting feature set a build may use:
`RequiredLinkFeatures(build)` MUST be ⊆ `LinkFeatures(P)`. If a program reaches
`molt_re_compile` but builds under `micro` (which excludes `stdlib_regex`), the
build refuses — because the user asked for a binary that *cannot* contain regex.
This is the *correct* meaning of "micro": "I want a small binary and I assert my
program needs no heavy domains." Reachability lets molt *prove* the assertion
instead of approximating it from imports. The refusal is now truthful: "your
program reaches `molt_re_compile` (a `stdlib_regex` intrinsic), which `micro`
excludes; either remove the reached regex usage or build with a profile that
includes `stdlib_regex`."

So a gratuitous `import re` with no reached call **builds fine on micro** (G2) —
because `stdlib_regex ∉ RequiredLinkFeatures`. Only an *actually reached* regex
call trips the ceiling. This is the entire bug-class cure.

### 5.2 Profile = archive selection ceiling + cache key

Runtime archives are expensive to build and are cached per feature-set
(`runtime_fingerprints.py`). The selected archive is the smallest cached archive
`A` with `RequiredLinkFeatures ⊆ LinkFeatures(A) ⊆ LinkFeatures(P)`. In
practice the cached tiers are exactly the Cargo ladder
(micro/edge/standard/server/full), so selection is: pick the **lowest ladder
tier whose link-affecting features ⊇ `RequiredLinkFeatures`**, capped at the
user's chosen profile. A gratuitous-`re` program on the default profile selects
`micro` (no heavy features reached) instead of dragging the program up a tier.
This is where the binary-size win is realized without per-program archive builds.

### 5.3 The drift cure (G4)

`_ALL_DOMAIN_FEATURES`/`_runtime_builtin_features_for_profile` stop being a
requirement oracle. `LinkFeatures(P)` is computed **directly from the Cargo
feature chain** (`tomllib.load(Cargo.toml)` + transitive expansion of the
profile feature), not a hand-maintained Python list. This is a small, exact
function (`profile_link_features(profile) -> frozenset[str]` resolving the
Cargo `[features]` graph), so the §1.3 drift (Python list ≠ Cargo chain) becomes
*structurally impossible*: there is one source (Cargo.toml) read by both the
ceiling check and the archive selection. `runtime_features.py`'s flat lists are
deleted or reduced to the builtin-feature (`builtin_set` etc.) plumbing that is
genuinely orthogonal to the stdlib ladder.

---

## 6. Migration path (import-driven → reachability-driven, no broken builds)

The migration is staged so that **each phase is itself a complete structural
piece** and the tree is never left with two live requirement authorities silently
disagreeing. Phases 0–1 are an atomic correctness arc (land together or with an
explicit hybrid-state baton note); 2–4 widen the structure.

### Phase 0 — Stop the bleeding *correctly*: fix the drift at its source (NOT a band-aid)

The §1.3 drift is a genuine bug independent of the redesign and blocks every
`re`/`itertools`/`difflib`/`xml`/`ipaddress`/`pathlib`-intrinsic program today.
The structurally correct Phase-0 is **make `LinkFeatures(profile)` read the
Cargo chain** (the §5.3 function) and route `_enforce_profile_feature_availability`
+ `runtime_build` feature selection through it, *deleting* the divergent
`_ALL_DOMAIN_FEATURES` mirror. This is not a workaround: it removes a duplicate
authority (Python list mirroring Cargo) and is a strict subset of the final
design (§5.3). After Phase 0, the gate is still import-driven (Fact A) but no
longer *drifts*; `import re` builds on `full`.

- Files: `src/molt/cli/runtime_features.py` (replace `_ALL_DOMAIN_FEATURES`
  flat list with `profile_link_features(profile)` reading
  `runtime/molt-runtime/Cargo.toml`); `src/molt/cli/module_stdlib_policy.py:140`
  (consume it); `src/molt/cli/runtime_build.py:278` (consume it).
- Test: extend `tests/cli/test_cli_profile_feature_refusal.py` with the missing
  coverage — `re`, `itertools`, `difflib`, `xml`, `ipaddress` MUST be buildable
  on `full` and refused on `micro`. These tests fail on main (proving the drift)
  and pass after Phase 0.
- Gate the Cargo-chain reader against drift: a test that asserts
  `profile_link_features("full")` ⊇ every `LINK_AFFECTING_FEATURE` the Cargo
  `stdlib_full` chain transitively enables (mechanically, from Cargo.toml), so a
  future Cargo edit cannot silently desync.

### Phase 1 — Introduce `RequiredFeatureSet` as a *shadow*, assert agreement

Add `required_features.py` computing `RequiredLinkFeatures` from the reachability
pass (§3). Wire it to run alongside the existing Fact-A gate but **not yet
authoritative**. Add a debug-gated assertion (the *only* sanctioned use of a
shadow assertion per CLAUDE.md — verifying an invariant *while* migrating, not
deferring it) that for every build, `RequiredLinkFeatures ⊆ FactA_required`
(reachability is never *more* than presence — a sanity direction that must always
hold) and log the *gap* (`FactA_required \ RequiredLinkFeatures`) which is
exactly the gratuitous set this design eliminates. This produces the §8
measurement data on real programs before flipping authority.

- New: `src/molt/cli/required_features.py` (Python side that invokes the TIR
  reachability pass and maps via `link_affecting_feature_gate_for_symbol`).
- The TIR pass already exists; expose `ReachedIntrinsics` to the CLI. There is a
  staging seam already: the backend computes the manifest during codegen
  (`compile_driver.rs:471`). Phase 1 adds a *pre-codegen analysis entry point*
  that runs the same `compute_intrinsic_manifest` over the TIR the frontend
  produces, so the CLI has `ReachedIntrinsics` before archive selection (§3.2
  pass 1). This is the structural heart; it must be a real shared call into
  `molt-tir`, not a Python re-implementation (one fact, §G1).

### Phase 2 — Flip authority for *selection*, keep profile as ceiling

Make `runtime_build` select the archive from `RequiredLinkFeatures` (§5.2),
capped at the profile. Now the gratuitous-`re` program on default profile selects
`micro`. The Fact-A gate is downgraded to the §3.4 thin ceiling check
(`RequiredLinkFeatures ⊆ LinkFeatures(profile)`), and the §4 dynamic-edge
refusal is added. Delete `_profile_feature_gap_for_module`'s role as the
requirement computer.

### Phase 3 — Delete the coarse requirement fact

Remove `module_required_intrinsic_names` from the *requirement* path entirely
(it may survive only as an *input* to the existing `_enforce_intrinsic_stdlib`
"is this module Python-only" policy, which is a different check). The single
requirement authority is now `RequiredLinkFeatures`. Remove
`_ensure_core_stdlib_modules`'s profile-coupling where it exists only to satisfy
the old gate. At this point Fact A no longer drives any build decision.

### Phase 4 — Smell-sweep (coordinated cleanup of the catalog's gratuitous imports)

Even with reachability-driven selection, the catalog's gratuitous imports are
*latent landmines* (a future refactor that reaches one line of a dead-imported
module would needlessly trip a ceiling) and are dead code besides. Clean them in
one coordinated change, applying the already-proven idioms (the `_pylong`
de-regex `f6e3793d5`; the lazy idiom in `pprint.py:524`/`difflib.py:787`/
`glob.py:43`; the ungated-intrinsic model in `fnmatch.py`):

- **Delete dead imports** (no behavior, pure removal):
  `importlib/metadata/__init__.py:18,19,28,32` (csv/email/re/zipfile — all dead,
  only in `__all__`); `importlib/metadata/_text.py:8` (re, dead);
  `logging/config.py:14` (struct, dead).
- **Lazy-import in rarely-called functions** (move module-level → function body;
  valid because non-entry stdlib is scanned `module_init`, which skips function
  bodies — `module_import_scanner.py:213`): `warnings.py:9` re → into
  `filterwarnings`; `unittest/__init__.py:9` re → into the regex-assert methods;
  `gettext.py:17` struct → into `_parse`; `typing_extensions.py:15` re → module
  `__getattr__` for `Match`/`Pattern`; `glob.py` `magic_check` → into the
  existing PEP 562 `__getattr__`.
- **De-regex trivial patterns**: `logging/config.py:13,33` `IDENTIFIER` →
  hand scan or the existing `molt_logging_config_valid_ident` intrinsic.
- **CORE (do not touch):** `textwrap`, `_strptime`, `json/*`, `_markupbase`,
  `symtable`, `xml/etree/ElementPath` tokenizer, `encodings/idna`, the
  self-referential family imports — these reach their intrinsics genuinely.

Phase 4 is *defense in depth*, not the cure (the cure is Phases 0–3). But it is
load-bearing for two reasons: (a) it removes dead code (CLAUDE.md: no dead
imports), and (b) under the *profile-ceiling* semantics (§5.1), a dead `import re`
in a stdlib module that the program *does* load means `re`'s module body runs at
import, executing its top-level `require_intrinsic("molt_re_compile")` — which IS
reached — so a *loaded* module with a gratuitous heavy import still forces the
feature. Phase 4 ensures loaded stdlib modules don't carry gratuitous top-level
heavy `require_intrinsic`s. (This is why §1.4's "module body executes → top-level
require_intrinsic is reached" matters: lazy-importing inside a rarely-called
function moves the `require_intrinsic` out of the always-run module body, so it
is reached only when the function is.)

> Phase 4 ordering caveat (correctness): a stdlib module that is *imported and
> whose body runs* will have its top-level `require_intrinsic` reached even after
> the redesign. So the lazy-idiom migrations in Phase 4 are what make a *loaded*
> `warnings`/`unittest` not force `stdlib_regex`. Phases 0–3 fix the *unloaded /
> dead-import* case; Phase 4 fixes the *loaded-but-feature-unused* case. Both are
> required for the full cure — this is the AST-to-binary spine: the same bug class
> has a frontend face (dead import) and a load-semantics face (eager module-body
> require_intrinsic), and both must close.

### Migration safety invariants

- After Phase 0: no program that built before regresses (Phase 0 only *widens*
  what `full` accepts and fixes the drift; it never narrows).
- After Phase 2: any build that the old gate *accepted* is still accepted (the
  ceiling check is ⊆ the old presence check, since reachability ⊆ presence). Any
  build the old gate *refused* is either still refused (genuinely reaches the
  feature) or now *accepted* (gratuitous import) — the intended improvement.
- The §4 dynamic-edge gate may refuse a *new* class (computed intrinsic name) —
  but that class is already broken at runtime today, so converting it to a
  compile-time refusal is a strict correctness improvement, not a regression.
  Audit: grep confirms zero first-party computed-name `require_intrinsic` call
  sites (all are literal), so the gate fires on no existing first-party code.

---

## 7. Verification plan

Every layer of the AST-to-binary spine gets a proof at its own layer.

### 7.1 Reachability-fact correctness (TIR layer)

- `reached_intrinsics_excludes_unreached_import`: a synthetic program
  `import re; print(1)` (no regex call) → `ReachedIntrinsics` contains no
  `molt_re_*` → `RequiredLinkFeatures` excludes `stdlib_regex`. The keystone
  regression for the whole bug class.
- `reached_intrinsics_includes_executed_module_body`: `import re` where re's
  body genuinely runs → `molt_re_compile` IS reached (the §1.4 invariant). Guards
  against over-aggressive elimination dropping a genuinely-loaded module's
  load-time requirement.
- `reached_intrinsics_includes_wrapper_and_field_indirection`: re-pin the
  existing `intrinsics_manifest.rs` data-flow-completeness cases
  (`_require_callable_intrinsic`, `_LazyIntrinsic` field stash) at the
  `RequiredLinkFeatures` level, so the requirement fact inherits the manifest's
  completeness guarantees.
- `nonconst_intrinsic_name_is_refused` (§4.1): a synthetic TIR with a computed
  (non-`const_str`) name into `require_intrinsic` → build refuses with the
  actionable message; prove the gate *fires* on this synthetic violation
  (negative control) and does NOT fire on the all-literal first-party stdlib
  (positive control — run over the real stdlib tree).

### 7.2 Feature-selection / ceiling correctness (CLI layer)

- `gratuitous_re_builds_on_micro`: the §1.1 program selects the `micro` archive
  and builds (the headline E2E win). Run as a real `molt build --target native`
  (E2E catches decomposition breaks per the project's hard-won lesson that unit
  tests pass atop a broken compiler).
- `reached_re_refused_on_micro_with_truthful_message`: a program with a *reached*
  `re.compile(...)` call on `micro` → refused, message names `molt_re_compile`
  and `stdlib_regex` and does NOT misdirect to a profile that lacks it.
- `profile_link_features_matches_cargo_chain` (§5.3): mechanically assert the
  Cargo-derived `LinkFeatures(full)` ⊇ every link-affecting feature in the Cargo
  `stdlib_full` transitive chain. Kills the §1.3 drift class.
- Port + strengthen `tests/cli/test_cli_profile_feature_refusal.py`: the
  existing module-granularity tests become *reachability*-granularity tests
  (a module imported but unreached no longer refuses; a module with a reached
  intrinsic does).

### 7.3 Binary-elimination correctness (link layer)

- `unreached_regex_crate_absent_from_binary`: build the gratuitous-`re` program;
  assert via `MOLT_DUMP_INTRINSIC_MANIFEST=1` (existing diagnostic,
  `app_resolver.rs:46`) that the manifest contains zero `molt_re_*`, and via
  `nm`/symbol inspection that no `molt_re_compile` is present. This proves the
  section is *unexpressible*, not merely smaller.
- The existing `runtime_intrinsic_symbols_required` fail-closed test stays green
  (the symbol-set precondition is unchanged).

### 7.4 No-regression across backends

- Parity: run the differential harness (`tests/molt_diff.py <files> --jobs 1`,
  serial per the memory note on parallel false-FAILs) on a corpus that imports
  heavy modules without reaching them; assert identical stdout/stderr to CPython
  and successful build on the *selected* (lower) tier.
- WASM parity: the WASM manifest scan
  (`wasm.rs` `manifest_intrinsic_names`) and native `compute_intrinsic_manifest`
  must agree (they already share the algorithm per `intrinsics_manifest.rs:5`).
  Add a test that both produce the same `ReachedIntrinsics` for a fixed program,
  so the §3 fact is backend-uniform (a native win never hides a WASM divergence).
- Luau/LLVM: `RequiredLinkFeatures` is target-independent (it is a property of
  the reached TIR); only archive availability per target differs, handled by the
  per-target `LinkFeatures(P)` ceiling.

---

## 8. Performance / size measurement plan (quantify the model)

Per the Performance Constitution, every claim reports
`benchmark → target → backend → profile → size → RSS → cold-start → command/log`.

### 8.1 The model (what to expect, before measuring)

- **Binary size:** dropping `stdlib_regex` removes the `molt-runtime-regex`
  crate from the link. `stdlib_serial`/`stdlib_email`/`stdlib_archive`/
  `stdlib_decimal` removals each drop their crate(s) + transitive deps
  (`Cargo.toml:89-119`: `dep:zip`, `dep:flate2`, `dep:rmpv`, etc.). The dominant
  win is *tier downgrade*: a program that today is forced to `standard`/`server`
  (because a gratuitous `import` dragged `serial`/`email`) now selects `micro`,
  shedding the entire delta between tiers.
- **Cold start:** smaller image → fewer pages to map / sign. Per the memory note,
  cold-start is an artifact-footprint/page-in problem, not a runtime-init problem
  (runtime init = 0.127ms), so this is the right lever for cold start.
- **Micro-buildability:** the binary outcome — programs that are *refused today*
  (drift, §1.3) or *forced up a tier today* (gratuitous import) become
  micro-buildable. This is a correctness/capability win measured as
  "builds: yes/no on micro" plus the size delta.

### 8.2 The measurement corpus

1. `import re; print("hi")` — the canonical instance. Expect: builds on micro;
   no `molt_re_*` in manifest; size = micro baseline.
2. The pyperformance subset programs, each measured for (a) selected tier under
   reachability vs (b) tier forced by presence today, with the size/RSS/cold-start
   delta. Many pure-compute benchmarks gratuitously pull `re` via
   `argparse`/`warnings` and should drop to a lower tier.
3. A genuinely regex-using program (control) — MUST stay on a regex-capable tier
   and MUST NOT regress in size/speed (the feature is genuinely needed).

### 8.3 Scoreboards (CI-gated)

- Add a **size scoreboard** row per corpus program: selected tier + binary size
  + manifest intrinsic count, under reachability-driven selection. A regression
  (program drifts *up* a tier, or manifest grows) is RED.
- The existing `MOLT_DUMP_INTRINSIC_MANIFEST` diagnostic is the deterministic,
  manifest-level signal (not just final binary size) — assert manifest contents
  in CI so a size regression is attributable to a specific newly-reached
  intrinsic, per the "fix the representation, not the peephole" posture.

### 8.4 Methodology

pyperf/pyperformance discipline: repeated workers, calibration, instability
detection, cold AND warm, JSON output. Classify each result GREEN / RED_STABLE /
RED_NOISY / TIE / DIMENSIONAL_WIN. The headline wins here are **DIMENSIONAL**
(binary size, cold start, buildability) and must be reported honestly as such —
not as warm-loop speed heals (a gratuitous-import fix does not change steady-state
loop throughput; it changes the artifact footprint and what builds at all).

---

## 9. Why this is the canonical (Lattner-grade) solution, not a patch

- **One fact, one authority.** The reachability closure already exists and is
  already trusted to decide the binary's contents. This design deletes the
  *second* fact (per-module presence) rather than reconciling two — eliminating
  the drift class structurally instead of pinning a mirror in sync (which would
  be the workaround). The duplicate-authority doctrine and the
  `runtime/molt-tir/` canonical-fact rule both point here.
- **The unit of elimination matches the semantics.** Intrinsic-granularity
  reachability is exactly right because module-load executes top-level
  `require_intrinsic` eagerly (§1.4); the design respects that invariant in both
  directions (never under-requires a loaded module, never over-requires an
  unloaded one).
- **Fail-closed on the unanalyzable frontier**, with the one sanctioned
  escape-hatch table (implicit edges) and a compile-time refusal for computed
  names — the GraalVM reachability-metadata / Nuitka implicit-imports / JS
  `sideEffects` model, adapted to molt's intrinsic sections. No silent drop, no
  undefined-symbol link error, no runtime "intrinsic unavailable".
- **Profiles become honest.** "micro" stops meaning "I imported only light
  modules" (an approximation the import graph cannot enforce) and starts meaning
  "my reached code needs no heavy domains" (a property reachability *proves*).
- **The whole AST-to-binary spine closes:** frontend dead-import face (Phases
  0–3) and load-semantics eager-require face (Phase 4) are recognized as one bug
  class with two faces and both are shut.

---

## 10. Exact change inventory (files / passes)

New:
- `src/molt/cli/required_features.py` — computes `RequiredLinkFeatures` from the
  reachability pass; the single requirement authority.
- `src/molt/cli/profile_link_features` (in `runtime_features.py` or a small new
  module) — resolves `LinkFeatures(profile)` from `runtime/molt-runtime/Cargo.toml`
  transitively. Deletes the `_ALL_DOMAIN_FEATURES` mirror as a requirement
  source.
- `runtime/molt-tir/` — pre-codegen analysis entry point exposing
  `ReachedIntrinsics` (reusing `compute_intrinsic_manifest`); plus
  `IMPLICIT_INTRINSIC_EDGES` table + `ImplicitClosure`.
- A TIR gate refusing non-`const_str` intrinsic names (§4.1).

Changed:
- `src/molt/cli/module_stdlib_policy.py:130` — `_enforce_profile_feature_availability`
  becomes the thin ceiling check (`RequiredLinkFeatures ⊆ LinkFeatures(profile)`)
  + the dynamic-edge refusal; drops per-module gap as requirement computer.
- `src/molt/cli/runtime_build.py:278` — archive selection from
  `RequiredLinkFeatures` capped at profile (§5.2), replacing flat profile
  expansion.
- `src/molt/cli/runtime_features.py:104,153` — `_ALL_DOMAIN_FEATURES` /
  `_runtime_builtin_features_for_profile` reduced to the genuinely-orthogonal
  builtin-feature plumbing; stdlib-ladder features come from Cargo.
- `src/molt/cli/module_graph.py:780` — call site wires the new authority; the
  reachability pass runs before archive selection.
- Stdlib smell-sweep files (Phase 4, §6).

Unchanged (correct as-is, gains an authoritative upstream):
- `runtime/molt-tir/src/passes/intrinsics_manifest.rs` (the reachability closure).
- `runtime/molt-backend/.../app_resolver.rs` (the per-app resolver / dead-strip).
- `runtime/molt-runtime/src/intrinsics/registry.rs` (the resolver registration).
- `runtime/molt-runtime/Cargo.toml` feature *groups* (sections are correct).
- `src/molt/_runtime_feature_gates.py` `RUNTIME_FEATURE_GATES` /
  `LINK_AFFECTING_FEATURES` (the symbol→section map remains the authority the
  new fact feeds through).

---

## 11. Risks and treatments

| Risk | Treatment |
|---|---|
| Reachability under-approximates (drops a genuinely-needed intrinsic) → undefined symbol / runtime "unavailable" | Impossible by construction for constant names (the manifest is data-flow-complete and validated against the staticlib symbol set); computed names are refused at compile time (§4.1); implicit edges are forced via the table (§4.2). Fail-closed default (§4.3). |
| Chicken-and-egg (selection needs the manifest, manifest validation needs a linked archive) | Two-pass over a cached full-superset symbol file (§3.2); membership-tests against the superset are exact; archives are cache lookups, not per-program builds. |
| §1.3 drift recurs via a future Cargo edit | `LinkFeatures(P)` reads Cargo directly (§5.3); `profile_link_features_matches_cargo_chain` gate (§7.2) fails CI on desync. |
| E2E breakage masked by green unit tests (the project's known failure mode) | Every selection/elimination claim is an E2E `molt build`, not a unit test (§7.2, §7.3). |
| WASM/native reachability divergence | They share the algorithm; §7.4 pins agreement with a cross-backend test. |
| Phase split leaves two live requirement authorities | Phases 0–1 are an atomic correctness arc; the Phase-1 shadow assertion proves agreement *before* Phase-2 flips authority; Phase 3 deletes Fact A. No hybrid requirement state ships silently. |
| A loaded stdlib module still forces a feature via eager top-level `require_intrinsic` | Recognized explicitly (§1.4, §6 caveat); Phase 4 lazy-idiom migrations move those out of always-run module bodies. The dead-import face and the load-semantics face are both closed. |

---

## 12. Definition of done

- A program that imports `re` (directly or transitively) without reaching a regex
  call builds on the `micro` profile, native and WASM, with no `molt_re_*` in the
  emitted manifest and no regex crate in the binary.
- A program that *reaches* a regex call on `micro` is refused with a truthful,
  actionable message naming the reached intrinsic and the real feature/profile
  that provides it.
- `RequiredLinkFeatures` is the single requirement authority; `LinkFeatures(P)`
  is Cargo-derived; the §1.3 drift class is gated out of existence.
- The smell-sweep (Phase 4) has removed every gratuitous import in the FIND
  catalog, using the proven dead-delete / lazy / de-regex / ungated-intrinsic
  idioms — no per-module special cases.
- Size + cold-start + micro-buildability scoreboards are green and CI-gated, with
  dimensional wins reported honestly as dimensional.
- All backends (native/Cranelift, WASM, LLVM, Luau) agree on `RequiredLinkFeatures`
  for a fixed program.

---

## Appendix A — Primary sources for the SOTA model

- GraalVM Native Image closed-world reachability + reachability-metadata +
  fail-closed (`MissingReflectionRegistrationError`, an uncatchable `Error`):
  https://www.graalvm.org/latest/reference-manual/native-image/metadata/ ,
  https://www.graalvm.org/latest/reference-manual/native-image/ (verified
  2026-06-26: missing metadata throws at runtime and "should not be caught,
  ensuring developers detect missing registrations during testing rather than in
  production" — molt converts this to a *compile-time* refusal, §4.1).
- Codon whole-program-from-`main` + LLVM globaldce/monomorphization (the
  closed-world-via-types model): https://www.exaloop.io/blog/mapping-python-to-llvm .
- Nuitka implicit/hidden-import database (`getImplicitImports`) — the curated
  unanalyzable-edge model mirrored by §4.2:
  https://github.com/Nuitka/Nuitka/blob/develop/nuitka/plugins/standard/ImplicitImports.py .
- Linker `--gc-sections` / `SHF_GNU_RETAIN` / `__start_/__stop_` retention /
  soundness (the binary last-mile molt already uses):
  https://maskray.me/blog/2021-02-28-linker-garbage-collection .
- Rust binary-size DCE levers (`build-std`, `panic=abort`, LTO) for the
  within-feature residue: https://github.com/johnthagen/min-sized-rust .
- JS tree-shaking `sideEffects` / `/*#__PURE__*/` / dynamic-import-as-boundary
  (retain-on-unprovable, never drop on a guess): https://webpack.js.org/guides/tree-shaking/ .

## Appendix B — The two facts, side by side (verified 2026-06-26)

| | Fact A (delete as requirement source) | Fact B (promote to authority) |
|---|---|---|
| Name | `module_required_intrinsic_names` | `compute_intrinsic_manifest` |
| File | `src/molt/stdlib_intrinsic_policy.py:97` | `runtime/molt-tir/src/passes/intrinsics_manifest.rs:61` |
| Granularity | per *module* (whole-file `ast.walk`) | per *reached `const_str`* across all TIR functions |
| Precision | over-approximate (presence) | exact (data-flow-complete for constant names) |
| Pipeline stage | static import-graph resolution (pre-codegen) | codegen (per-app manifest), liftable pre-codegen (§3.2) |
| Sees dead/unloaded module imports? | YES (the bug) | NO (the cure) |
| Validated against linked symbols? | NO | YES (`runtime_intrinsic_symbols_required`, fail-closed) |
| Drives today | the refusal + (via profile model) selection | the per-app resolver + linker dead-strip |
| Drives after | nothing (removed) | refusal + selection + dead-strip (one fact) |
