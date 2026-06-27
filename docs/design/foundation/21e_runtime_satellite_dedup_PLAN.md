<!-- Update 2026-06-27: the math-family, XML, difflib, and ipaddress fallback
lanes have been deleted. The path satellite now owns the event-specific
capability audit bridge, so `os_ext`/`pathlib` leaf code no longer emits generic
`path.has_capability` events or skips gates for process/env/path operations.
The live satellite parity guard tracks 4 remaining dual-path pairs with a
residual ceiling of 886: functions_logging=89, itertools=273, os_ext=277,
pathlib=247. cmath_mod, colorsys, fractions, math, random_mod, xml_etree,
xml_sax, difflib, and ipaddress are no longer PAIRS entries. Their leaf crates
own the Rust implementations and generated leaf resolver modules. -->

<!-- Foundation blueprint 21e. Architect: portfolio-architect (Plan agent), 2026-06-24.
Executable plan for decomposition move #4 — the molt-runtime satellite dedup arc (R.2 + R.3).
Supersedes doc 21 §1.4/§2.4 prose AND the missing memory/recovery/baton_move_R_satellite_drift.md
(this doc is the authoritative R-arc record). Verified against live HEAD 2d788293a. Design only. -->

# 21e — molt-runtime Satellite Dedup Arc (R.2 + R.3), executable plan

## PREFACE — premise drifted from doc 21 (verified live, absorb before executing)
1. **24 pairs, not 28.** `builtins/mod.rs` has 24 `#[cfg(not(feature="stdlib_*"))]` gates; `tools/check_satellite_parity.py` PAIRS has 24. `zoneinfo` already deleted (`e4f9300bf`) — one R.3-style deletion already landed (the template). ~3 others were never dual-path (leaf-only — see #6).
2. **The R.1 parity guard is RED at committed HEAD.** `python tools/check_satellite_parity.py` fails: ipaddress 0→6, fractions 0→2, os_ext 286→345, decimal 2→12; total residual **2193 > ceiling 2116**. Committed drift (confirmed via git stash — satellite files unmodified vs HEAD). **R.2/R.3 cannot start on a red oracle → this is Step R.0.**
3. **doc 21's "12 pairs at zero residual" is stale** — live baseline shows **7** zero-residual: cmath_mod, difflib, fractions, functions_zipfile, ipaddress, xml_etree, xml_sax. doc 21's named list is wrong (stringprep/html/unicodedata/zoneinfo no longer in PAIRS; http has 381 residual). Always use live `--verbose`, never doc 21's list.
4. **The R.2 access-unification abstraction ALREADY EXISTS and is piloted.** `molt-runtime-core` provides `CoreGilToken (= PyToken)`, `with_core_gil!`, `with_gil_entry_body!`, `RuntimeVtable`, `prelude`. **`molt-runtime-serial` is a working `RuntimeVtable` pilot.** R.2 is therefore "generalize the serial vtable pattern to the other 15 satellites," NOT green-field — far lower risk than doc 21 implies.
5. **The recovery baton `memory/recovery/baton_move_R_satellite_drift.md` does not exist** (only genthrow_51/, takeover_20260609/). Every cross-ref to it dangles. 21e supersedes it.
6. **Scope boundary:** net, asyncio, collections, stringprep, text are LEAF-ONLY (no in-tree mod fallback) → out of scope. Only the 24 dual-path PAIRS are in scope.

## 1. CURRENT-STATE MAP (verified)
### 1.1 Live remaining pairs after R.3 deletions and path-audit reconciliation

As of 2026-06-27, `python tools/check_satellite_parity.py --verbose` reports:

| pair | residual | satellite | remaining structural work |
|---|--:|---|---|
| functions_logging | 89 | logging/http family | reconcile logging extension state and delete/own the remaining fallback lane |
| itertools | 273 | itertools | reconcile iterator/runtime-state drift before any deletion |
| os_ext | 277 | path | remaining drift is dir-fd helper locality, WASM/sysconf/list-allocation source shape, and source spelling around the new path audit bridge |
| pathlib | 247 | path | remaining drift is mostly source spelling around the new path audit helper; behavior now uses event-specific `pathlib.*` audit names |

The current ratchet ceiling is 886. `os_ext`/`pathlib` are still dual-path and
must not be deleted until residual reaches zero, but the access-policy authority
behind the path satellite is now event-specific rather than the old generic
`path.has_capability` bridge side effect.

### 1.1 Historical 24-pair map from the initial 21e audit
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
(Bold = drifted past baseline → currently failing.)

### 1.2 Dual-path mechanism
Same downstream symbol namespace fed by one of two sources per feature: in-tree (`#[cfg(not(feature="stdlib_X"))] pub(crate) mod X;` in builtins/mod.rs + `pub use crate::builtins::X::*` in lib.rs) vs satellite (`#[cfg(feature="stdlib_X")] pub use molt_runtime_X::X::*` in lib.rs:752+). Both re-export into the SAME molt-runtime namespace → resolver/callers identical, only the source crate differs by feature. An R.3 deletion is a pure source swap, no resolver changes (zoneinfo precedent confirms).

### 1.3 Tier → copy mapping
Feature chain micro ⊂ edge ⊂ standard ⊂ server ⊂ full; default profile = micro (cli). `default=["stdlib_full",...]` → the default native build already activates every satellite, so R.3's "default includes the satellite" is already satisfied for default; the real constraint is the REDUCED tiers (micro/edge/standard) that omit satellites for binary size, plus the WASM feature set. Deleting an in-tree copy forces its satellite ON in those tiers (zoneinfo added stdlib_zoneinfo to categories.toml feature attribution and regenerated _runtime_feature_gates.py).

### 1.4 The TWO access models (the R.2 problem)
- **Direct-call (in-tree)** functions_http.rs:1-26: `use crate::{...}` directly; `PyToken` from concurrency::GilGuard; `crate::with_gil_entry_nopanic!`.
- **FFI-bridge (satellite)** molt-runtime-http/functions_http.rs:1-22: `use molt_runtime_core::prelude::*` + `crate::bridge::{...}` FFI shims; `CoreGilToken` from CoreGilGuard; `with_core_gil!`.
- **Two bridge sub-architectures coexist:** per-symbol `extern "C"` (15 of 16 satellites — http_bridge.rs=56 shims, math=56, path=25) vs **`RuntimeVtable` single-dispatch (serial only — the pilot):** serial fetches one `&'static RuntimeVtable` via the single extern `__molt_serial_get_vtable()`; serial_bridge.rs:740-812 defines the 66-field vtable + getter; serial's bridge has 2 no_mangle vs http's 56.
- **Token types ALREADY unified:** core lib.rs:265 `pub type CoreGilToken = PyToken;`; serial bridge fns take `_py: &PyToken` (in-tree shape). Residual divergence = (a) macro name, (b) import block, (c) token-threading (normalized by RT_WRAPPER_EQUIVALENTS).

### 1.5 decimal is special: builtins/decimal.rs is a 13-line dispatcher (`#[cfg(molt_has_mpdec)]`→with_mpdec else without); guard compares satellite vs decimal_without_mpdec.rs; `stdlib_decimal=["stdlib_math"]`. Do decimal LAST, bespoke (preserve the mpdec split or have the satellite absorb both).

### 1.6 The R.1 guard invariant (R.2/R.3 must preserve): tools/check_satellite_parity.py + tests/satellite_parity.rs. Per pair: normalize access-layer diffs (_strip_use_blocks, GIL macros→__GIL__!, tokens→__TOK__, strip crate::/bridge::/molt_runtime_core:: prefixes, collapse unsafe{}, strip #[cfg(test)]+comments), compare sorted line-multiset symmetric difference. FAIL if any pair's residual > baseline, content-hash differs at equal count, a pair is missing, or total > ratchet_ceiling (one-way ratchet). R.2 changes to canonical access spelling MUST update the normalizer in the SAME commit.

## 2. R.0 — UNBLOCK THE GUARD (prerequisite, do first)
Reconciliation in the spirit of R.1 (NOT new R.2/R.3). Steps (each its own commit; guard-green at end):
1. Triage the 4 regressions: `check_satellite_parity.py --show <pair>` for ipaddress, fractions, os_ext, decimal (`<`=in-tree-only, `>`=satellite-only).
2. Port each one-sided fix into the OTHER copy (behavior-preserving; the guard is the oracle, the differential suite tests/differential/stdlib/ is the proof — run on BOTH tiers: default=satellite, MOLT_STDLIB_PROFILE=micro=in-tree). Never pick a side blindly. Add a differential regression passing on both tiers.
3. Re-ratchet: `--update-baseline` (it refuses to RAISE the ceiling; bring total ≤ 2116, ideally lower).
4. Gate: `cargo test -p molt-runtime --features stdlib_full --test satellite_parity` green; the 4 pairs at committed counts or lower.
Parallelizable: 4 reconciliations touch disjoint pairs → 4 workers; the baseline re-ratchet is one serializing commit.

## 3. R.2 — ACCESS-LAYER UNIFICATION (one source compiles in both contexts)
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
Per-step gate (every commit): `check_satellite_parity.py` green (residual not grown; extend normalizer in the SAME commit if canonical spelling changes + re-ratchet); `cargo build -p molt-runtime --no-default-features --features stdlib_micro` AND `--features stdlib_full` both compile; `cargo build -p molt-runtime-<crate>` standalone; differential suite green on BOTH tiers; nm/symbol check (per-symbol externs → one vtable getter; the satellite-side G5).

## 4. R.3 — PER-SATELLITE DEDUP (delete the in-tree copy, no compat shim)
**Two shapes (zoneinfo precedent `e4f9300bf` = outright delete, recommended):**
- **Option A (outright delete, recommended):** delete builtins/X.rs; remove the `#[cfg(not())] mod X;` + the `not()` re-export (keep only the `feature` re-export); make stdlib_X mandatory for the tiers that used the in-tree copy (add to stdlib_micro/edge/standard as needed + categories.toml feature attribution + generated _runtime_feature_gates.py + resolver output); update the CLI profile-feature gate (_enforce_profile_feature_availability/CAPABILITY_PROFILES in cli/__init__.py + tests/cli/test_cli_profile_feature_refusal.py); remove the pair from PAIRS + re-ratchet (total drops by its residual, must be 0).
- **Option B (`#[path]` single-source):** in-tree `mod X` becomes `#[cfg(not(feature="stdlib_X"))] #[path="../../../molt-runtime-X/src/X.rs"] mod X;` (needs R.2's facade). Keeps source single without forcing the satellite into micro.
- **Decision rule:** A for light modules; B for modules whose satellite drags heavy deps (rustls/mio/serde_json) into micro/edge. Check `cargo tree -f '{p} {f}'` first (doc 21's feature-unification trap).

### 4.2 R.3 ordering (lowest-risk first; HARD PRECONDITION: residual 0 before any deletion)
1. functions_zipfile (0, serial) — validates the recipe. 2. Other zero-residual: cmath_mod, difflib, ipaddress(post-R.0), fractions(post-R.0), xml_etree, xml_sax. 3. Low-residual after reconciliation to 0: binascii, base64_mod, datetime, colorsys, functions_email, regex. 4. High-residual (reconcile to 0 first): functions_logging, structs, csv, math, configparser, random_mod, itertools(re-verify post-267df44e9), pathlib, os_ext(post-R.0), functions_http(reconcile the stdlib_net-gated blocks). 5. decimal LAST + bespoke.
Per-step gate: guard green + pair removed from PAIRS + ceiling ratcheted down; both build contexts compile (reduced tier + stdlib_full + WASM if applicable); differential green every tier; binary-size delta measured (A should be ≤0; if a satellite drags heavy deps into a reduced tier, use B); `cargo tree` diff (no unintended feature flip); no new duplication (grep).

## 5. ORDERING SUMMARY
R.0 fix red guard (ipaddress/fractions/os_ext/decimal + re-ratchet) [4+1 commits] → R.2a access facade + unified macro [1] → R.2b serial family through facade [3-5] → R.2c single-source functions_zipfile [1] → R.2d per-symbol satellites to vtable simplest→hardest [~15] → R.3 per-pair dedup residual-0, light/zero first, decimal last [~24]. R.0 unblocks the oracle; R.2 precedes every R.3 (one source must satisfy both ABIs first); serial pilots R.2 (already on the target vtable). **Swarm parallelism:** R.2d + R.3 parcel per-crate EXCEPT shared serializing files — check_satellite_parity.py (PAIRS, normalizer) + satellite_parity_baseline.json (one owner re-ratchets), lib.rs re-export block + builtins/mod.rs (merge contention), the RuntimeVtable struct (additive, coordinate).

## 6. RISKS
Guard already red → R.0 first. Reconcile-by-picking-a-side drops behavior → guard is oracle, differential on BOTH tiers is proof, never delete a copy with residual>0. Two bridge architectures → generalize the proven serial vtable, not a third shim. Normalizer must track canonical spelling → update GIL_MACROS/PREFIXES/RT_WRAPPER_EQUIVALENTS/TOKEN_TYPES in the same commit. One source two ABIs → the access facade is the seam (standalone=extern vtable, in-tree=direct); validate both builds every commit. Heavy-dep leak into micro → cargo tree before each deletion, Option B for heavy. decimal mpdec split → bespoke, last. itertools RuntimeState slots → re-verify post-267df44e9. functions_http residual is mostly stdlib_net-gated ctypes/urllib → reconcile into satellite respecting the net gate. Shared serializing files → one baseline owner. Dangling baton → 21e supersedes it.

## 7. ALREADY DONE (do not redo)
R.1 landed: guard (230789ab5), csv reconcile (3b2ac0129), doc correction (8ded12e06), itertools slot-scoping (267df44e9), binascii/http sweep (f95225814), satellite clippy gate (ff22a06c7). R.3 deletions have landed for zoneinfo (e4f9300bf, the template), the math family, XML, difflib, and ipaddress. Path access reconciliation has landed an event-specific audit bridge for `molt-runtime-path`; remaining `os_ext`/`pathlib` drift is no longer a missing capability-gate or generic-audit problem. R.2 abstraction partially exists: molt-runtime-core (CoreGilToken/with_core_gil!/with_gil_entry_body!/RuntimeVtable/prelude); molt-runtime-serial is the working vtable pilot. Differential infra exists.

## Critical files
tools/check_satellite_parity.py (oracle: PAIRS/normalizer/ratchet); runtime/molt-runtime-core/src/lib.rs (R.2 facade home — add `access`); runtime/molt-runtime-serial/src/bridge.rs + runtime/molt-runtime/src/serial_bridge.rs (the RuntimeVtable pilot); runtime/molt-runtime/src/{lib.rs,builtins/mod.rs} (dual-path re-export wiring); runtime/molt-runtime/Cargo.toml + runtime/molt-runtime/src/intrinsics/categories.toml + generated src/molt/_runtime_feature_gates.py + src/molt/cli/__init__.py (tier/feature gating, zoneinfo precedent e4f9300bf).
