# Tkinter Lowering Plan (Intrinsic-First, Cross-Platform, CPython 3.12+)

**Status:** In progress (Phase-0 intrinsic skeleton + focused deterministic differential coverage landed; native runtime now uses owned dynamic Tcl FFI with `useTk=False` app creation and native `loadtk` support; `tkinter.ttk` now includes broad constructor + common-method forwarding wrappers validated under headless intrinsic stubs)
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
- `tkinter.ttk` Phase-0 wrappers now include constructor coverage plus thin
  forwarding for common widget/style APIs (`Button/Checkbutton/Radiobutton.invoke`,
  `Combobox.current/set`, `Entry.bbox/identify/validate`, notebook/panedwindow/
  progressbar/scale/spinbox/treeview method families, and `Style` lookup/layout/
  theme/element calls) with deterministic headless-stub verification.
- Dialog-query lowering now has dedicated Rust intrinsic entry points
  (`molt_tk_dialog_show`, `molt_tk_simpledialog_query`) with `tkinter.dialog` and
  `tkinter.simpledialog` reduced to thin argument-wiring wrappers.
- Native `molt_tk_simpledialog_query` now drives a real modal entry flow in Rust
  (`toplevel` + `entry` + `OK/Cancel` + `vwait` loop + validation/bell/retry),
  with deterministic headless behavior retained only for wasm/non-native lanes.

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
   `tests/differential/stdlib/tkinter_phase0_core_semantics.py` for Phase-0
   `_tkinter`/`tkinter` import + missing-attribute contracts, `_tkinter`
   core API surface checks, and wrapper-level submodule import/error-shape/
   capability-gate checks (`tkinter.__main__`, dialog/helper stubs, and
   `tkinter.ttk`) without requiring a real GUI backend. `ttk` coverage now
   includes constructor/alias export checks and common thin-forwarding method
   routing assertions under headless intrinsic stubs.
   Keep deep `tkinter.ttk` value-conversion, callback binding, and true Tk
   behavior parity validation in a follow-on tranche.
2. Capability-denied and capability-missing tests (native + wasm).
3. Import-failure shape tests for unsupported hosts/targets.
4. Memory and determinism checks under differential harness constraints.

### Required execution environment
Use external-volume artifacts for all heavy runs:
```bash
export MOLT_EXT_ROOT=/Volumes/APDataStore/Molt
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
