<!-- Update 2026-06-27: every tracked in-tree/satellite two-copy runtime stdlib
pair has been deleted or single-sourced. The live satellite parity guard tracks
0 remaining dual-path pairs with residual ceiling 0. Reduced builds now compile
leaf-owned satellite source by direct include where a reduced-tier fallback is
still needed: `functions_logging`, `itertools`, `os_ext`, and `pathlib` all use
the leaf crate source instead of a second in-tree implementation. -->

<!-- Foundation blueprint 21e. Architect: portfolio-architect (Plan agent), 2026-06-24.
Executable plan for decomposition move #4 — the molt-runtime satellite dedup arc (R.2 + R.3).
Supersedes doc 21 §1.4/§2.4 prose AND the missing memory/recovery/baton_move_R_satellite_drift.md
(this doc is the authoritative R-arc record). The original execution plan is
retained below as archived context; the live 2026-06-27 status is zero tracked
dual-path pairs with a zero residual ceiling. -->

# 21e — molt-runtime Satellite Dedup Arc (R.2 + R.3), zero-pair status

## PREFACE — live status after completion
1. **Zero tracked dual-path pairs.** `tools/check_satellite_parity.py` has an empty `PAIRS` table and `tools/satellite_parity_baseline.json` has residual ceiling 0.
2. **The parity guard is green at zero.** `python tools/check_satellite_parity.py --verbose` reports 0 pairs and total residual 0. Any future in-tree/satellite pair is new authority debt and must be declared explicitly.
3. **Reduced-tier fallbacks are single-source.** The remaining fallback paths that still need reduced-tier source (`functions_logging`, `itertools`, `os_ext`, and `pathlib`) compile leaf-owned Rust source by direct `#[path]` include rather than keeping a second in-tree implementation.
4. **The R.2 access-unification abstraction still matters for future satellites.** `molt-runtime-core` provides `CoreGilToken (= PyToken)`, `with_core_gil!`, `with_gil_entry_body!`, `RuntimeVtable`, and `prelude`; `molt-runtime-serial` remains the RuntimeVtable pilot.
5. **The recovery baton `memory/recovery/baton_move_R_satellite_drift.md` does not exist** (only genthrow_51/, takeover_20260609/). Every cross-ref to it dangles. 21e supersedes it.
6. **The historical sections below are an archive, not a live work queue.** Use the live guard, checked-in source, and current generated artifacts before deriving new work from old residual counts.

## 1. CURRENT-STATE MAP (verified)
### 1.1 Live remaining pairs after R.3 deletions

As of 2026-06-27, `python tools/check_satellite_parity.py --verbose` reports:

| pair | residual | satellite | remaining structural work |
|---|--:|---|---|
| none | 0 | n/a | PAIRS is empty; reintroducing a two-copy pair is new debt and must be justified as an explicit authority decision |

The current ratchet ceiling is 0. The remaining reduced-tier fallbacks are
single-source `#[path]` direct includes of satellite-owned Rust files, not
separate in-tree implementations.

### 1.1 Historical 24-pair map from the initial 21e audit (archived)
| pair | residual | satellite | gate |
|---|--:|---|---|
| cmath_mod,difflib,functions_zipfile,xml_etree,xml_sax | 0 | math/difflib/serial/xml/xml | resp. |
| fractions | **2**(was 0) | math | not stdlib_math |
| binascii | 2 | serial | not stdlib_serial |
| ipaddress | **6**(was 0) | ipaddress | not stdlib_ipaddress |
| decimal | **12**(was 2) | serial | special (§1.5) |
| datetime 16, colorsys 18, functions_email 26, regex 32, base64_mod 4, structs 105, csv 144, math 153, configparser 159, random_mod 171 | (as shown) | serial/math/regex/... | resp. |
| functions_logging 67 | | http | not stdlib_http |
| itertools 273, pathlib 277 | | itertools/path | resp. |
| os_ext | **345**(was 286) | path | not stdlib_path |
| functions_http | 381 | http | not stdlib_http |
(Archived counts from the initial audit. They are not current status.)

### 1.2 Historical dual-path mechanism
The original failure mode was the same downstream symbol namespace fed by one of two sources per feature: in-tree (`#[cfg(not(feature="stdlib_X"))] pub(crate) mod X;` in builtins/mod.rs + `pub use crate::builtins::X::*` in lib.rs) vs satellite (`#[cfg(feature="stdlib_X")] pub use molt_runtime_X::X::*`). The tracked copy pairs are now gone. Current reduced-tier fallbacks must stay single-source direct includes or leaf-only routes; reintroducing a second physical implementation is new debt.

### 1.3 Tier → copy mapping
Feature chain micro ⊂ edge ⊂ standard ⊂ server ⊂ full; default profile = micro (cli). `default=["stdlib_full",...]` → the default native build already activates every satellite, so R.3's "default includes the satellite" is already satisfied for default; the real constraint is the REDUCED tiers (micro/edge/standard) that omit satellites for binary size, plus the WASM feature set. Deleting an in-tree copy forces its satellite ON in those tiers (zoneinfo added stdlib_zoneinfo to categories.toml feature attribution and regenerated _runtime_feature_gates.py).

### 1.4 The TWO access models (the R.2 problem)
- **Direct-call (in-tree)** functions_http.rs:1-26: `use crate::{...}` directly; `PyToken` from concurrency::GilGuard; `crate::with_gil_entry_nopanic!`.
- **FFI-bridge (satellite)** molt-runtime-http/functions_http.rs:1-22: `use molt_runtime_core::prelude::*` + `crate::bridge::{...}` FFI shims; `CoreGilToken` from CoreGilGuard; `with_core_gil!`.
- **Two bridge sub-architectures coexist:** per-symbol `extern "C"` (15 of 16 satellites — http_bridge.rs=56 shims, math=56, path=25) vs **`RuntimeVtable` single-dispatch (serial only — the pilot):** serial fetches one `&'static RuntimeVtable` via the single extern `__molt_serial_get_vtable()`; serial_bridge.rs:740-812 defines the 66-field vtable + getter; serial's bridge has 2 no_mangle vs http's 56.
- **Token types ALREADY unified:** core lib.rs:265 `pub type CoreGilToken = PyToken;`; serial bridge fns take `_py: &PyToken` (in-tree shape). Residual divergence = (a) macro name, (b) import block, (c) token-threading (normalized by RT_WRAPPER_EQUIVALENTS).

### 1.5 decimal is special: builtins/decimal.rs is a 13-line dispatcher (`#[cfg(molt_has_mpdec)]`→with_mpdec else without); guard compares satellite vs decimal_without_mpdec.rs; `stdlib_decimal=["stdlib_math"]`. Do decimal LAST, bespoke (preserve the mpdec split or have the satellite absorb both).

### 1.6 The R.1 guard invariant (R.2/R.3 must preserve): tools/check_satellite_parity.py + tests/satellite_parity.rs. Per pair: normalize access-layer diffs (_strip_use_blocks, GIL macros→__GIL__!, tokens→__TOK__, strip crate::/bridge::/molt_runtime_core:: prefixes, collapse unsafe{}, strip #[cfg(test)]+comments), compare sorted line-multiset symmetric difference. FAIL if any pair's residual > baseline, content-hash differs at equal count, a pair is missing, or total > ratchet_ceiling (one-way ratchet). R.2 changes to canonical access spelling MUST update the normalizer in the SAME commit. `tools/check_runtime_symbol_owners.py` is the sibling satellite-link guard: every `#[no_mangle] extern "C"` symbol may have only one satellite-crate owner under `stdlib_full`, so accidental cross-satellite duplicates fail before the linker.

## 2. R.0 — UNBLOCK THE GUARD (archived; complete for tracked pairs)
Reconciliation in the spirit of R.1 (NOT new R.2/R.3). Steps (each its own commit; guard-green at end):
1. Triage the 4 regressions: `check_satellite_parity.py --show <pair>` for ipaddress, fractions, os_ext, decimal (`<`=in-tree-only, `>`=satellite-only).
2. Port each one-sided fix into the OTHER copy (behavior-preserving; the guard is the oracle, the differential suite tests/differential/stdlib/ is the proof — run on BOTH tiers: default=satellite, MOLT_STDLIB_PROFILE=micro=in-tree). Never pick a side blindly. Add a differential regression passing on both tiers.
3. Re-ratchet: `--update-baseline` (it refuses to RAISE the ceiling; bring total ≤ 2116, ideally lower).
4. Gate: `cargo test -p molt-runtime --features stdlib_full --test satellite_parity` green; the 4 pairs at committed counts or lower.
Parallelizable: 4 reconciliations touch disjoint pairs → 4 workers; the baseline re-ratchet is one serializing commit.

## 3. R.2 — ACCESS-LAYER UNIFICATION (archived design pattern)
**Design decision: generalize the serial RuntimeVtable; do NOT invent a new shim.** The single source of truth per module is the SATELLITE file, written against a UNIFIED ACCESS FACADE that cfg-resolves to (a) RuntimeVtable FFI dispatch standalone, (b) direct `crate::` calls when compiled inside molt-runtime. Vtable is the right target: collapses the FFI surface (serial 1 extern getter + 66 fn-ptrs vs http 56 no_mangle), already takes `&PyToken`.

Three pieces (all build on existing code):
1. **`molt_runtime_core::access` facade (NEW in molt-runtime-core):** free fns/trait = the single call surface module source uses (`access::alloc_string(py,...)`, `access::raise_exception(...)`). Two cfg impls: standalone → RuntimeVtable `vt()` (serial pattern generalized); in-crate (a `molt_runtime_in_tree` cfg set by molt-runtime when it `#[path]`-includes the file) → `#[inline]` wrappers over direct `crate::` calls + real GIL token.
2. **Unified GIL-entry macro:** collapse `with_gil_entry_nopanic!` + `with_core_gil!` to ONE prelude macro cfg-dispatching its guard source (CoreGilGuard standalone, concurrency::GilGuard in-tree). Body sees `let $py=&token;` identically. with_gil_entry_body! already centralizes the panic contract.
3. **Per-satellite vtable migration (the bulk):** for each of the 15 per-symbol satellites, do what serial did — define a RuntimeVtable static + single getter in molt-runtime/src/<crate>_bridge.rs, rewrite the satellite bridge.rs to dispatch through vt(), route module source through the access facade. Behavior-preserving (each migrated bridge fn wraps the exact internal call the old __molt_<crate>_<fn> shim wrapped).

### 3.3 The concrete divergences R.2 removes
| divergence | in-tree | satellite | resolution |
|---|---|---|---|
| internal calls | crate::alloc_string | crate::bridge::alloc_string | access::alloc_string (cfg) |
| GIL macro | with_gil_entry_nopanic! | with_core_gil! | one prelude macro (cfg guard) |
| token | PyToken | CoreGilToken(=PyToken) | already unified |
| import root | use crate::{} | molt_runtime_core::prelude::* + crate::bridge::* | source imports only molt_runtime_core::{prelude::*,access::*} |
| token threading | helper takes _py | bridge acquires GIL, drops _py | uniform facade sig; in-tree uses it, vtable ignores like serial |
| unsafe{} | safe direct | unsafe{extern} | facade hides unsafe in standalone impl |
After R.2, files are identical modulo a single cfg-selected facade import → precondition for R.3 `#[path]` single-source.

### 3.4 R.2 ordering + per-step gate
Order: **serial family first** (already on vtable — only needs facade + unified macro: csv/structs/configparser/datetime/base64_mod/binascii/functions_email/functions_zipfile/decimal); prove single-source on functions_zipfile (residual 0). Then per-symbol satellites simplest→hardest by no_mangle count: difflib→ipaddress→logging→text→itertools→regex→crypto/path/collections→http/math.
Per-step gate (every commit): `check_satellite_parity.py` green (residual not grown; extend normalizer in the SAME commit if canonical spelling changes + re-ratchet); `tools/check_runtime_symbol_owners.py` green (no duplicate satellite owners); `cargo build -p molt-runtime --no-default-features --features stdlib_micro` AND `--features stdlib_full` both compile; `cargo build -p molt-runtime-<crate>` standalone; differential suite green on BOTH tiers; nm/symbol check (per-symbol externs → one vtable getter; the satellite-side G5).

## 4. R.3 — PER-SATELLITE DEDUP (archived recipe; tracked pairs complete)
**Two shapes (zoneinfo precedent `e4f9300bf` = outright delete, recommended):**
- **Option A (outright delete, recommended):** delete builtins/X.rs; remove the `#[cfg(not())] mod X;` + the `not()` re-export (keep only the `feature` re-export); make stdlib_X mandatory for the tiers that used the in-tree copy (add to stdlib_micro/edge/standard as needed + categories.toml feature attribution + generated _runtime_feature_gates.py + resolver output); update the CLI profile-feature gate (_enforce_profile_feature_availability/CAPABILITY_PROFILES in cli/__init__.py + tests/cli/test_cli_profile_feature_refusal.py); remove the pair from PAIRS + re-ratchet (total drops by its residual, must be 0).
- **Option B (`#[path]` single-source):** in-tree `mod X` becomes `#[cfg(not(feature="stdlib_X"))] #[path="../../../molt-runtime-X/src/X.rs"] mod X;` (needs R.2's facade). Keeps source single without forcing the satellite into micro.
- **Decision rule:** A for light modules; B for modules whose satellite drags heavy deps (rustls/mio/serde_json) into micro/edge. Check `cargo tree -f '{p} {f}'` first (doc 21's feature-unification trap).

### 4.2 R.3 ordering (closed for tracked pairs)
The tracked R.3 pair list is complete: PAIRS is empty and the residual ceiling is
0. Future satellite decomposition work must start by proving whether a proposed
fallback is a single-source direct include or a new two-copy authority. A new
two-copy authority must be added to PAIRS intentionally, guarded immediately,
and retired back to zero before it can be treated as stable architecture.

Per-step gate for any future reintroduction: guard green + pair removed from
PAIRS + ceiling ratcheted back to zero; both build contexts compile (reduced
tier + stdlib_full + WASM if applicable); differential green every tier; binary
size and cargo-tree deltas measured when a reduced tier changes; no new
duplication.

## 5. HISTORICAL ORDERING SUMMARY
R.0 fix red guard (ipaddress/fractions/os_ext/decimal + re-ratchet) [4+1 commits] → R.2a access facade + unified macro [1] → R.2b serial family through facade [3-5] → R.2c single-source functions_zipfile [1] → R.2d per-symbol satellites to vtable simplest→hardest [~15] → R.3 per-pair dedup residual-0, light/zero first, decimal last [~24]. R.0 unblocks the oracle; R.2 precedes every R.3 (one source must satisfy both ABIs first); serial pilots R.2 (already on the target vtable). **Swarm parallelism:** R.2d + R.3 parcel per-crate EXCEPT shared serializing files — check_satellite_parity.py (PAIRS, normalizer) + satellite_parity_baseline.json (one owner re-ratchets), lib.rs re-export block + builtins/mod.rs (merge contention), the RuntimeVtable struct (additive, coordinate).

## 6. RISKS
Current risks: accidental reintroduction of a two-copy fallback, direct-include
bridge facade growth, and reduced/full build skew around the shared leaf source.
The guard is green and zero-pair; if a future pair is added, PAIRS plus the
baseline must move in the same change and retire back to zero. Validate both
reduced and full builds whenever a direct-include fallback or satellite bridge
helper changes. If `runtime/molt-runtime/src/bridge.rs` grows another helper
family, split it behind submodules while keeping the `crate::bridge::*` facade
for leaf sources.

## 7. ALREADY DONE (do not redo)
R.1 landed: guard (230789ab5), csv reconcile (3b2ac0129), doc correction (8ded12e06), itertools slot-scoping (267df44e9), binascii/http sweep (f95225814), satellite clippy gate (ff22a06c7). R.3 deletions have landed for zoneinfo (e4f9300bf, the template), the math family, XML, difflib, ipaddress, path (`os_ext`/`pathlib` via direct include), `functions_logging`, and `itertools`. The guard's tracked dual-authority surface is now empty with residual ceiling 0. R.2 abstraction partially exists: molt-runtime-core (CoreGilToken/with_core_gil!/with_gil_entry_body!/RuntimeVtable/prelude); molt-runtime-serial is the working vtable pilot. Differential infra exists.

## Critical files
tools/check_satellite_parity.py (oracle: PAIRS/normalizer/ratchet); runtime/molt-runtime-core/src/lib.rs (R.2 facade home — add `access`); runtime/molt-runtime-serial/src/bridge.rs + runtime/molt-runtime/src/serial_bridge.rs (the RuntimeVtable pilot); runtime/molt-runtime/src/{lib.rs,builtins/mod.rs} (dual-path re-export wiring); runtime/molt-runtime/Cargo.toml + runtime/molt-runtime/src/intrinsics/categories.toml + generated src/molt/_runtime_feature_gates.py + src/molt/cli/__init__.py (tier/feature gating, zoneinfo precedent e4f9300bf).
