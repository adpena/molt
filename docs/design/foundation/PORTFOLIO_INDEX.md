<!-- The 100-year-plan portfolio: the dependency DAG + execution order across all
foundation blueprints. Authored 2026-06-24. A map, not a plan -- each node links to its
executable blueprint. Governed by DESIGN_DOCTRINE.md. -->

# 100-Year Plan — Portfolio Integration Index (the execution DAG)

Every node below is an executable foundation blueprint. This index gives the **dependency
DAG** (which arc unblocks which) and the **execution order**, so the parallel swarm has one
map. All arcs are checkable against [DESIGN_DOCTRINE.md](DESIGN_DOCTRINE.md): retire a
god-file killer; satisfy both the Pythonista and Rustacean lenses; fix a representation.

## The nodes
- **Doctrine:** [DESIGN_DOCTRINE](DESIGN_DOCTRINE.md) — governs all.
- **Decomposition (the god-file spine):** [21a](21a_function_compiler_function_split_PLAN.md) function-split · [21b](21b_crate_graph_blueprint.md) crate-graph (molt-ir←molt-passes←molt-lower, per-backend crates, codegen-abi) · [21c](21c_frontend_mixin_decomposition_PLAN.md) frontend mixins · [21d](21d_cli_package_decomposition_PLAN.md) cli/ package · [21e](21e_runtime_satellite_dedup_PLAN.md) runtime satellite dedup.
- **Infra (Lane C):** [64](64_perf_scoreboards_and_harness.md) perf scoreboards+harness · [59](59_semantic_fact_plane.md) semantic fact-plane.
- **Perf frontier (Lane B):** [65](65_perf_compression_ladder.md) compression-ladder (9 rungs) · [63](63_deforestation_fusion.md) deforestation/fusion · [60](60_tree_shaking_whole_program_dce.md) tree-shaking · [61](61_binary_size_and_output_optimization.md) binary-size · [62](62_startup_cold_start.md) startup/cold-start.
- **Safety (Lane A, P0):** [55](55_memory_safety_ownership_lattice.md) ownership-lattice.
- **Compat:** [66](66_compat_cpython_parity.md) CPython parity · [67](67_compat_tinygrad_dflash.md) tinygrad/DFlash.
- **Throughput / DX / UX / demos:** [54](54_throughput_concurrency_async.md) · [56](56_dx_buildspeed_tooling.md) · [57](57_ux_cli_errors_onboarding.md) · [58](58_killer_demos.md).

## The dependency DAG (→ = "unblocks / feeds")
```
DESIGN_DOCTRINE ─ governs ─▶ everything

64 scoreboards ─┐                                  (Rung 0 = measurement)
59 fact-plane ──┴─▶ 65 compression-ladder ──▶ the perf product
                     │  Rung 1 ◀── 55 ownership-lattice (Perceus/escape/RC)
                     │  Rung 2 ◀── call-facts (47) / IP summaries
                     │  Rung 4 = ShapeFacts  ← THE GREENFIELD HOLE (no shape system yet)
                     │  Rung 5 = SIMD ◀── Rung 3 Repr lanes
                     │  Rung 6 = 63 deforestation/fusion ──▶ fewer reachable helpers
                     │  Rung 7 = portable-IR backend parity (66)
                     └  Rung 8 = footprint = 60 tree-shaking ─▶ 61 binary-size ─▶ 62 startup

60 tree-shaking ─▶ 61 binary-size (Size = 6th projection over 64) ─▶ 62 startup (smaller cold tail)
55 ownership-lattice ─▶ 54 throughput (GIL-free safety) + 65 Rung 1 + 63 K3 (escape)
59 fact-plane ─▶ 66 parity (one registry authority; backends lower, never decide) + every generated fact
21b crate-graph ─▶ DX incremental builds (56) ; 21a/c/d/e ─▶ god-file ownership-collision kills
66 parity + 67 ML + 65 perf + 54 throughput + 56 DX ─▶ 58 killer demos
```

## Execution order (council three-lane; A blocks B only on memory-unsafety)
1. **Lane A — P0 safety FIRST where unsafe:** 21e **R.0** (the satellite parity guard is RED — reconcile before its R.2/R.3); 55 ownership-lattice (rung-1→2 bridge; the resurrection/finalizer SIGSEGV outranks all perf).
2. **Lane C — infra, concurrent from day one:** 64 scoreboards (wire the perf gate into CI — the named gap) + 59 fact-plane generators (op_kinds/protocol, gen-X --check). These are the substrate every B arc steers by. 59 also fixes the structural_audit god_files metric so it credits cohesive decomposition products (not re-pin).
3. **Lane B — perf frontier, rung-ordered:** 65 Rung 1 (←55) ‖ Rung 0 (64) → Rung 2 → Rung 3 Repr → **Rung 4 ShapeFacts (build the hole)** → Rung 5 SIMD → Rung 6 (63) → Rung 7 parity → Rung 8 (60→61→62 footprint).
4. **Decomposition spine — continuous (the swarm is here):** 21a families → 21b S1–S8 crate splits (S1 molt-ir first, the gate) → 21c/21d/21e. Each split is a green move-only commit; the structural_audit ratchet only goes down.
5. **Compat + product, on the substrate:** 66 parity (the differential oracle, all 4 backends) + 67 tinygrad/DFlash (exact fidelity) → 54 throughput → 56 DX / 57 UX → 58 demos (the switch-makers).

## Invariant for every arc + PR (from the doctrine)
Retire a god-file killer? · Exact CPython semantics AND Pythonic feel? · Fix a representation (an IR fact that makes a class unexpressible), not a symptom? · One generated authority (drift uncompilable)? · Memory safety structural (a lattice proof)? · Measured across target × backend × profile vs CPython/PyPy/Codon, cold AND warm?
