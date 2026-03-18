# Tkinter Lowering Plan (Intrinsic-First, Cross-Platform, CPython 3.12+)

**Status:** In progress (Phase-0 bootstrap tranche is complete; native runtime now uses owned dynamic Tcl FFI with `useTk=False` app creation and native `loadtk` support; headless Rust runtime lowers broad `tkinter.ttk` plus core `bind/event/wm/winfo/layout` command families with callback substitution dispatch; focused runtime-semantics differential checks are landed while broader parity remains in progress)
**Owner:** stdlib + runtime + compiler + tooling + testing
**Scope:** `_tkinter` + `tkinter` stdlib family on native targets (`linux`,
`macos`, `windows`) with explicit capability-gated behavior on wasm targets.

## Mission
Ship production-grade Tkinter support for compiled Molt binaries without host
Python fallback, using Rust runtime primitives plus intrinsic-backed Python
shims.

## Feasibility Decision
Tkinter support is feasible for native targets and should be built as a staged,
intrinsic-first program.

For wasm targets, full Tk/Tcl parity is not currently feasible. The contract is
explicit capability-gated absence (deterministic error behavior) until a
separate browser UI backend is designed and accepted.

## Design Choice: Rust Bindings + Thin API Shims
Recommended architecture (aligned with Molt policy):
1. Implement Tk/Tcl runtime integration in Rust (`molt-runtime`).
2. Expose stable intrinsic entry points via
   `runtime/molt-runtime/src/intrinsics/manifest.pyi`.
3. Keep `_tkinter.py` and `tkinter/*.py` as thin wrappers for argument
   normalization, object shells, exception mapping, and capability checks.

Non-goal:
- Do not rely on host CPython `_tkinter` extension loading at runtime.
- Do not add Python-side behavior fallbacks that bypass missing intrinsics.

## API Transpilation Strategy
User-facing `tkinter` API surface can be accelerated with generated wrappers,
but behavior ownership remains in Rust intrinsics:
1. Generate shim class/method skeletons from CPython `tkinter` signatures for
   repetitive API coverage.
2. Route every behavior path through intrinsic calls (no direct host fallback).
3. Keep generated Python code limited to argument coercion, object shells, and
   exception mapping.
4. Reuse generated wrappers for `ttk`/dialog modules where signatures are
   stable; hand-write only the semantic edge cases.

Current implementation note:
- Native Tk runtime uses an owned dynamic Tcl C-API loader (`libloading`) instead
  of depending on unstable third-party wrapper crates.
- `molt_tk_app_new` now honors `useTk=False` (Tcl-only creation path), and
  native `loadtk` is idempotently routed through `package require Tk`.
- Native runtime feature plumbing is default-on for non-wasm CLI builds;
  set `MOLT_RUNTIME_TK_NATIVE=0` to force the non-native fallback lane.
- `quit` currently exits Molt’s Tk mainloop without force-destroying the app
  handle, aligning closer to CPython `_tkinter` lifecycle behavior.
- `tkinter.ttk` wrappers now cover constructor and common widget/style
  forwarding paths, and headless Rust runtime command
  lowering now implements these major families for compiled execution:
  `state`/`instate`/`identify` core widget semantics, `invoke`,
  `Combobox.current/set`, `Entry.bbox/identify/validate`, notebook and
  panedwindow container operations, progress/scale/spinbox helpers, Treeview
  semantics, and top-level `ttk::style` + `ttk::notebook::enableTraversal`.
- Headless `event generate` now dispatches stored bind scripts through Rust
  callback routing with `%`-placeholder substitution (`%x/%y/%D/%K/%A/%#/%T/%X/%Y`),
  and `tkinter.Misc.bind` now requests CPython-style substitution payloads so
  callback event-object fidelity is preserved across headless/native lanes.
- Headless runtime callback fidelity work now covers `after`/`trace`/`tkwait`
  semantics end-to-end (`after cancel` token/script cleanup, variable-trace
  callback dispatch ordering, and `wait_variable` progress through queued
  events) in Rust runtime command handlers.
- Headless/runtime `ttk.Treeview` semantics now enforce structural invariants
  (duplicate-child rejection and cycle-safe move/children reparenting), and
  `event generate` dispatch now includes `Treeview.tag_bind` script lanes for
  tag-scoped callback firing fidelity.
- Dialog/runtime parity tightening in Rust now includes command-specific
  commondialog option validation (unknown-option rejection),
  Tcl-compatible boolean prefix parsing for dialog booleans (for example
  `t/f/y/n`), preserved filedialog whitespace in headless result synthesis, and
  native-after token bookkeeping/cleanup so `after cancel` correctly drops
  pending one-shot callback state.
- Variable-trace registrations now preserve deterministic insertion ordering in
  Rust (`trace add`/`trace remove`/`trace info`/callback dispatch), avoiding
  hash-order drift across callback-mode combinations.
- Trace lifecycle cleanup is now Rust-owned end-to-end:
  `molt_tk_trace_clear` removes all registrations for a variable and releases
  callback commands, while trace callback dispatch now treats callback strings
  as Tcl command prefixes (multi-word callback prefixes preserve appended
  `name/index/op` arguments correctly).
- `ttk.Treeview` parity tightening now rejects previously silent generic
  fallback subcommands with explicit Tcl errors, validates `insert`/`move`
  indices strictly (`int|end`), and raises deterministic errors for unknown
  item ids in `selection {set,add,remove,toggle}` operations.
- `ttk.Notebook`/`ttk.Panedwindow` container `insert` now validates index
  tokens strictly (`int|end`) and raises deterministic Tcl-style errors for
  invalid index strings instead of silently treating them as `end`.
- `ttk.Notebook.index()` now enforces numeric bounds for explicit integer
  indices (`Slave index <n> out of bounds`) while preserving `end` and
  managed-tab identifier lookup behavior.
- Core widget command lowering now replaces several prior generic no-op lanes
  with Rust-owned stateful semantics:
  `Menu.add/insert/delete/entrycget/entryconfigure/type/post/unpost/invoke/xposition/yposition/tk_popup`,
  `PanedWindow.add/insert/forget/panes/panecget/paneconfigure`,
  `Listbox.itemcget/itemconfigure` (with per-item option-state reindexing),
  and `Text.replace/edit/dump` (including callback-command dispatch for
  `dump -command` and command-prefix invocation for menu/button `invoke`).
- Non-native `tk_popup` top-level command dispatch is now explicit in Rust
  command routing (instead of falling into unknown-command behavior), with
  deterministic menu-post state updates and optional active-entry targeting.
- Live smoke coverage now includes OS-specific filehandler readiness behavior
  (`createfilehandler` with real pipe FDs on linux/macos; Windows
  `NotImplementedError` contract) together with runtime checks for `after`,
  `trace`, and `wait_variable` callback fidelity.
- Dedicated live filehandler smoke lanes are now split out per OS
  (`test_tkinter_live_filehandler_smoke_{linux,macos,windows}`) so FD-readiness
  regressions can be isolated from broader live GUI probes.
- `molt_tk_commondialog_show` now uses a strict supported-command allowlist and
  runtime-owned dispatch (`native` routes to real Tk, and non-native lanes use
  deterministic headless semantics for the supported command family).
- Dedicated Rust intrinsics now back higher-level dialog families:
  `molt_tk_messagebox_show` and `molt_tk_filedialog_show` route
  `tkinter.messagebox`/`tkinter.filedialog` through runtime-owned command
  dispatch, and the non-native lane lowers deterministic headless results for
  `tk_messageBox`, `tk_getOpenFile`, `tk_getSaveFile`, `tk_chooseDirectory`,
  and `tk_chooseColor`.
- Frontend direct-call allowlisting now includes `tkinter._support` helper
  aliases so `tkinter.ttk` capability gates compile in Tier-0 mode; a focused
  CLI compile regression test covers `import tkinter.ttk` + `ttk.Frame`.
- Dialog-query lowering now has dedicated Rust intrinsic entry points
  (`molt_tk_dialog_show`, `molt_tk_simpledialog_query`) with `tkinter.dialog` and
  `tkinter.simpledialog` reduced to thin argument-wiring wrappers.
- Native `molt_tk_simpledialog_query` now drives a real modal entry flow in Rust
  (`toplevel` + `entry` + `OK/Cancel` + `vwait` loop + validation/bell/retry),
  with deterministic headless behavior retained only for wasm/non-native lanes.
- CPython version gating is explicit at the import boundary: `tkinter.tix` is
  available for 3.12 and treated as absent (`ModuleNotFoundError`) for 3.13+.

## Cross-Platform Contract
| Target | Contract | Notes |
| --- | --- | --- |
| `native/linux` | Supported (when Tk runtime available) | Require Tk runtime presence; CI smoke under Xvfb and headless-friendly checks. |
| `native/macos` | Supported (when Tk runtime available) | Enforce main-thread UI ownership and Cocoa-compatible loop integration. |
| `native/windows` | Supported (when Tk runtime available) | Enforce Win32 message-pump integration; package/link policy must keep startup deterministic. |
| `wasm_wasi` | Explicitly unsupported initially | Import/use must fail deterministically with capability/availability error contract. |
| `wasm_browser` | Explicitly unsupported initially | Same deterministic unsupported contract; no hidden DOM fallback. |

## Architecture
### 1) Runtime Tk subsystem (Rust)
- Add a dedicated runtime module boundary (for example, `runtime/molt-runtime/src/gui/tk/`)
  for Tk interpreter lifecycle, event dispatch, and callback marshalling.
- Keep `lib.rs` thin; re-export from focused modules only.
- Maintain deterministic state transitions for create, run, callback, destroy.

### 2) Intrinsic API boundary
Minimum phase-1 intrinsic families:
- Availability and bootstrap:
  - `molt_tk_available`
  - `molt_tk_app_new`
  - `molt_tk_quit`
- Event loop:
  - `molt_tk_mainloop`
  - `molt_tk_do_one_event`
  - `molt_tk_after`
- Widget and command plumbing:
  - `molt_tk_call`
  - `molt_tk_bind_command`
  - `molt_tk_destroy_widget`
- Introspection and errors:
  - `molt_tk_last_error`

Phase-2 adds typed helpers for high-traffic paths (`Button`, `Label`,
`Entry`, `Frame`, `Canvas`, `ttk` core controls) to reduce call overhead and
improve diagnostics.

### 3) Python stdlib surfaces
- Replace stubs in:
  - `src/molt/stdlib/_tkinter.py`
  - `src/molt/stdlib/tkinter/__init__.py`
  - `src/molt/stdlib/tkinter/ttk.py`
  - selected submodules (`messagebox`, `filedialog`, `simpledialog`,
    `font`, `constants`) in phase order.
- Keep shims thin: no host-Python imports, no dynamic fallback behavior.
- Preserve CPython exception class/message shape where supported.

### 4) Import and availability policy
- If Tk is unavailable on a target/host, `_tkinter` and `tkinter` must fail in
  a deterministic, documented way from the import boundary.
- Use runtime-known absence/intrinsic checks; do not allow late ad-hoc failures
  deep in widget operations for predictable unsupported cases.

### 5) Event-loop integration
- Integrate Tk pump with Molt runtime scheduler through explicit run-once hooks
  (`molt_tk_do_one_event`) instead of unmanaged host loop ownership.
- Maintain main-thread affinity for Tk operations.
- Support callback re-entry into Python through GIL-safe runtime call paths.

### 6) Capability model
New capability namespace for GUI support:
- `gui.window` for window creation/mainloop
- `gui.clipboard` for clipboard operations
- reuse existing `process.spawn` gating where Tk paths require child process use

Default stance:
- Non-trusted mode requires explicit capability grants.
- Trusted mode can bypass capability checks as currently defined by policy.

## Build, Packaging, and Toolchain Policy
### Native
- Define deterministic discovery/link policy for Tk/Tcl per OS:
  - Linux: system Tk/Tcl lookup with explicit error if missing.
  - macOS: framework/path resolution with main-thread guarantees.
  - Windows: DLL resolution rules and packaging constraints documented.
- Support reproducible builds by pinning/recording Tk ABI assumptions.

### Wasm
- Keep `_tkinter`/`tkinter` in explicit unsupported state until a separate UI
  backend contract exists.
- No silent fallback to browser DOM APIs or host Python bridges.

## Differential and Validation Plan
### Required evidence for each phase
1. Targeted native differential tests against CPython behavior for supported
   subset (`tests/differential/stdlib/tkinter_*`), including
   `tests/differential/stdlib/tkinter_phase0_core_semantics.py` for
   bootstrap-and-runtime `_tkinter`/`tkinter` import + missing-attribute
   contracts, `_tkinter` core API surface checks, and wrapper-level submodule
   import/error-shape/capability-gate checks (`tkinter.__main__`, dialog/helper
   wrappers, and `tkinter.ttk`) without requiring a real GUI backend. Coverage
   now includes runtime-lowered headless semantics checks for both core Tk and
   ttk lanes (`tkinter:runtime_core_semantics`,
   `tkinter.ttk:runtime_semantics`) across bind/event, wm/winfo/layout, and
   Style/Notebook/Panedwindow/common-widget paths.
   Keep deep `tkinter.ttk` value-conversion, callback binding, and true Tk
   behavior parity validation in a follow-on tranche.
   Focused tkinter differential reruns currently require lock/watchdog-quiet
   lanes plus elevated `MOLT_DIFF_BUILD_TIMEOUT` to avoid build-time aborts
   during concurrent runtime rebuild contention.
2. Capability-denied and capability-missing tests (native + wasm).
3. Import-failure shape tests for unsupported hosts/targets.
4. Memory and determinism checks under differential harness constraints.

### Required execution environment
Use canonical artifact roots for all heavy runs:
```bash
export MOLT_EXT_ROOT=/path/to/artifact-root  # optional external root
export CARGO_TARGET_DIR=$MOLT_EXT_ROOT/cargo-target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$MOLT_EXT_ROOT/molt_cache
export MOLT_DIFF_ROOT=$MOLT_EXT_ROOT/diff
export MOLT_DIFF_TMPDIR=$MOLT_EXT_ROOT/tmp
export TMPDIR=$MOLT_EXT_ROOT/tmp
export MOLT_DIFF_MEASURE_RSS=1
export MOLT_DIFF_RLIMIT_GB=10
```

## Phased Rollout
| Phase | Goal | Exit Criteria |
| --- | --- | --- |
| 0 | Runtime + intrinsic skeleton | `_tkinter` imports in native when available, deterministic import failure otherwise, no host fallback paths. |
| 1 | Core widget loop | `Tk`, `mainloop`, `after`, basic widget creation/events pass targeted differentials on linux/macos/windows. |
| 2 | `ttk` and dialogs tranche | `tkinter.ttk`, `messagebox`, `simpledialog`, `filedialog` core semantics validated with parity tests (constructor/common-forwarding lane landed; deep behavior parity in progress). |
| 3 | Hardening and docs promotion | Matrix/status/availability docs updated to partial/supported with evidence and gating notes. |

## Open Risks
1. Main-thread UI constraints differ across OSs and can deadlock if loop
   ownership is unclear.
2. Tk installation/linking variance can break reproducibility without strict
   discovery policy.
3. Callback marshalling can create re-entrancy hazards if GIL/runtime ordering
   is not explicit.
4. Wasm behavior must remain explicitly unsupported until a documented backend
   exists; accidental partial support would create nondeterministic divergence.

## Canonical Files To Update During Implementation
- `runtime/molt-runtime/src/intrinsics/manifest.pyi`
- `runtime/molt-runtime/src/intrinsics/generated.rs` (generated)
- `src/molt/_intrinsics.pyi` (generated)
- `src/molt/stdlib/_tkinter.py`
- `src/molt/stdlib/tkinter/*.py`
- `docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md`
- `docs/spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md`
- `docs/spec/STATUS.md`
- `ROADMAP.md`
