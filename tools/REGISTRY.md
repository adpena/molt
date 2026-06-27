# Tool Registry — the high-signal tools, run by default. DO NOT REBUILD THESE.

**STOP.** Before you build any analysis / verification / perf / audit tool, it
almost certainly already exists. This is the canonical, agent-facing catalog of
molt's highest-signal tools. Agents kept rediscovering and rebuilding these
(two agents independently re-authored `perf_authority.py`; the float and int
repr cuts were built twice from opposite ends) — that waste ends here.

## The one command

```
uv run --python 3.12 python3 tools/ci_gate.py            # run ALL gates (tier 1)
uv run --python 3.12 python3 tools/ci_gate.py --tier all # + tier 2/3
```

`tools/ci_gate.py` is THE default verification entry point: it runs every wired
gate below. If you want "is my change good?", run this — do not re-implement a
checker it already runs. Its `_build_checks()` IS the live registry of gates;
this file is the human/agent-readable index of the whole tool surface.

## Catalog (by purpose)

### Verify / drift-gates (wired in ci_gate tier-1 — fail closed)
- `tools/structural_audit.py --check` - duplicate-authority / kitchen-sink and undecomposed-god-file ratchet.
- `tools/build_graph_audit.py --check` - recompile-blast-radius ratchet (doc 56 FACT-A): fails if a cross-crate dep re-couples a layer (back-edge) or widens a crate's downstream recompile cone past `tools/build_graph_baseline.json`. Declared DAG = `runtime/crate_graph.toml`; reads `cargo metadata` only (no compile). `--write-board` -> `docs/design/foundation/BUILD_GRAPH_BOARD.md`.
- `tools/check_runtime_symbol_owners.py` - one `#[no_mangle] extern "C"` owner per runtime satellite symbol.
- `tools/check_perf_gate_wiring.py` — the canonical perf gate must fire on main.
- `tools/check_ratio_direction.py` — no raw `t/t` ratio outside the signed authority.
- `tools/check_perf_freshness.py` — no stale/unstamped perf doc with citable numbers.
- `tools/audit_op_kinds.py` / `tools/gen_op_kinds.py --check` — op-kind authority + drift.
- `cargo test op_family` — native dispatch↔handler disjointness (drift unexpressible).

### Measure / perf (the CANONICAL source of truth — cite only these)
- `tools/perf_scoreboard.py --profile release-fast --classify` — the ONLY citable
  perf board (CPython floor, cold+warm, quiescent, provenance-gated). Non-canonical
  lanes (`bench.py`, `bench/harness.py`) self-stamp `authoritative=false`.
- `tools/perf_board.py <scoreboard.json>` — project the source board into the FIVE
  gated boards (CPython/Backend/Profile/PyPy/Codon), each its own artifact + exit
  code (doc 64 §3.2). A native win cannot hide a wasm regression. Pure consumer.
- `tools/perf_history.py <board.json>... --gate [--record]` — board-vs-history
  regression gate (doc 64 Phase 4): fails on a previously-green cell that
  regressed; only authoritative boards become baselines (Rule 2).
- `tools/check_perf_plane_gate.py` — falsifiable self-proof that the plane gate
  FAILS on a synthetic CPython-red (ci_gate Tier 1; a gate that cannot fail is
  vacuous).
- `molt.metric_ratios.signed_ratio` — the SOLE ratio authority (explicit direction).
- `tools/perf_causality.py` + `tools/pass_delta_dashboard.py` — attribute a CPython-RED
  to its missing IR fact (do not re-derive perf causality by hand).
- `tools/dx_build_timer.py` — build wall-clock (prime/cold/incremental/test-lib).

### Audit / repr (the evidence engine)
- `runtime/molt-backend/src/bin/typed_repr_report.rs` — per-function scalar-repr +
  alloc-site audit (the substrate for the coupled Repr-trace / molt-check).
- `runtime/molt-backend/src/tir/verify_lir_repr.rs` — register-passability invariant.

### Build / run (always use these, never a raw binary)
- `python -m molt build --release ...` — the perf-gate build profile (default `dev`
  is unoptimized, NOT the perf gate). Needs `.venv/Scripts/python.exe` on this host.
- `tools/safe_run.py --rss-mb N --timeout S -- <binary>` — the ONLY safe way to run a
  raw molt binary (hard RSS+walltime caps; raw `./binary` can OOM the host).
- `tests/molt_diff.py <files> --jobs 1` — the CPython differential (serial until the
  collective-budget-pool lands; parallel gives false FAILs).

### Runtime safety / process custody
- `tools/memory_guard.py` / `tools/process_sentinel.py` — process custody; the
  classifier authority is `tools/memory_guard_core/process_model.py` (Codex/Claude/
  host-control-plane are NEVER kill targets). Never name/tree-kill.

### Durable design authorities (read before re-designing)
- `docs/design/meta_bug_taxonomy.md` — the meta-bug fix queue (proxy-measurement class).
- `docs/design/foundation/` — the 100-year executable plans (21a-e, 54-67).
- The memory index (`MEMORY.md`) — load-bearing facts not derivable from a quick read.

## The discoverability contract (drift-gate — enforced, not aspirational)
A tool is "real" only when it is (1) **wired** (run by `ci_gate` / a workflow /
the build pipeline), (2) **fast** (under its tier wall-clock budget), and (3)
**registered here**. A `check_tool_registry` gate (ci_gate tier-1) fails closed if
a high-signal `tools/*.py` / diagnostic bin is not listed here and run by the
entry point — so a potent tool can never again go undiscovered, unrun, or rebuilt.
(Inventory completion + the gate land from the wiring audit `w3esr1q6s`.)
