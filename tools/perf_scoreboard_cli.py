"""Command-line orchestration for the canonical performance scoreboard."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys
from collections.abc import Mapping
from typing import Any

from perf_scoreboard_model import Cell


def main(api: Mapping[str, Any], argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="CPython floor-scoreboard — the release-blocking perf gate."
    )
    parser.add_argument(
        "--set",
        default="core",
        choices=["core", "smoke"],
        help="benchmark set (default: core = the curated verified subset)",
    )
    parser.add_argument(
        "--benchmark",
        action="append",
        default=None,
        help="explicit benchmark path/key (repeatable); overrides --set",
    )
    parser.add_argument(
        "--backend",
        action="append",
        default=None,
        choices=["native", "llvm", "wasm"],
        help="backend lane(s) to measure (default: native llvm)",
    )
    parser.add_argument(
        "--profile",
        action="append",
        default=None,
        choices=["release-fast", "release-output", "dev-fast"],
        help="profile(s) to measure (default: release-fast)",
    )
    parser.add_argument(
        "--cpython",
        default=None,
        help="CPython oracle binary (default: system python3, e.g. 3.14 — NOT the venv)",
    )
    parser.add_argument("--samples", type=int, default=api["DEFAULT_SAMPLES"])
    parser.add_argument("--warmup", type=int, default=api["DEFAULT_WARMUP"])
    parser.add_argument("--rss-mb", type=int, default=api["DEFAULT_RUN_RSS_MB"])
    parser.add_argument("--timeout", type=float, default=api["DEFAULT_RUN_TIMEOUT_S"])
    # --- #69 measurement-hygiene flags ------------------------------------
    parser.add_argument(
        "--require-quiescent",
        action="store_true",
        help=(
            "BEFORE measuring, detect contamination (active cargo/rustc/molt "
            "builds [codex excluded], 1-min load > ncpu*0.5, runnable-thread "
            "storm, thermal throttle). A non-quiet machine stamps the board "
            "authoritative=false and prints NON-AUTHORITATIVE; the run still "
            "produces EXPLORATORY numbers, never authoritative warm verdicts."
        ),
    )
    parser.add_argument(
        "--quiescence-wait-s",
        type=float,
        default=0.0,
        help=(
            "when --require-quiescent is set, wait up to this many seconds for "
            "the same quiescence predicate to pass before failing closed"
        ),
    )
    parser.add_argument(
        "--quiescence-poll-s",
        type=float,
        default=15.0,
        help="poll cadence for --quiescence-wait-s (default: 15s)",
    )
    parser.add_argument(
        "--print-provenance",
        action="store_true",
        help=(
            "emit the full provenance block (origin/candidate SHA, dirty, daemon, "
            "stdlib cache key, backend binary identity, cold/warm, repeat/variance "
            "+ the NEW quiescence fields active_molt_processes / "
            "active_cargo_or_rustc_processes / loadavg_1m / ncpu / runnable_signal)"
        ),
    )
    parser.add_argument(
        "--repeat",
        type=int,
        default=1,
        help=(
            "N independent measurement PASSES per cell; compute median + variance "
            "+ a 95%% CI. A verdict is STABLE only if the CI does not straddle "
            "1.00 across passes (default 1 = single pass, no CI)."
        ),
    )
    parser.add_argument(
        "--classify",
        action="store_true",
        help=(
            "replace the single warm verdict with the council's 5 states: "
            "RED_STABLE / RED_NOISY / TIE / GREEN_STABLE / DIMENSIONAL_WIN "
            "(+ INFRA). RED_STABLE (quiescent+stable+CI-below-1.0) is the TRUE "
            "warm-red set. DIMENSIONAL_WIN needs --baseline."
        ),
    )
    parser.add_argument(
        "--emit-cycle-profile",
        action="store_true",
        help=(
            "for warm reds, capture a CYCLE profile (/usr/bin/sample self-time) "
            "and attach the top symbols — the Rule-1 attribution signal (CYCLES, "
            "not alloc-count). Falls back to a documented note if unavailable."
        ),
    )
    # --- #76 warm-hot cycle attribution -----------------------------------
    parser.add_argument(
        "--sample-hot-only",
        action="store_true",
        help=(
            "WARM-HOT cycle attribution (#76): for each benchmark build a LOOPED "
            "(--inner-repeat) + SYMBOLICATED (MOLT_KEEP_SYMBOLS=1) variant, sample "
            "its STEADY STATE, and report the top IN-BINARY hot frames. Defeats "
            "the one-shot launch/page-in (_dyld_start) domination that makes warm "
            "attribution impossible. REFUSES (no hot-path claim) if launch still "
            ">= 40%% of leaf self-time after looping. Writes a JSON profile cell; "
            "does NOT run the speedup gate (use without --classify)."
        ),
    )
    parser.add_argument(
        "--inner-repeat",
        type=int,
        default=api["DEFAULT_INNER_REPEAT"],
        metavar="N",
        help=(
            "inner-repeat factor N for --sample-hot-only: wrap the benchmark "
            "main() in `for _ in range(N): main()` INSIDE one process so launch/"
            f"page-in amortizes (pyperf inner_loops model). Default {api['DEFAULT_INNER_REPEAT']}. "
            "Semantics-preserving (refused if the benchmark is not loopable)."
        ),
    )
    parser.add_argument(
        "--profile-build",
        action="store_true",
        help=(
            "(implied by --sample-hot-only) build benchmarks with molt user-fn "
            "symbols retained (MOLT_KEEP_SYMBOLS=1) so sample/Instruments attribute "
            "to real functions instead of ???. Additive: never changes the normal "
            "stripped product build or any speedup measurement."
        ),
    )
    parser.add_argument(
        "--out",
        default=None,
        help="output JSON path (default: bench/scoreboard/cpython_<gitrev>.json)",
    )
    parser.add_argument(
        "--baseline",
        nargs="?",
        const="__latest__",
        default=None,
        help="diff against a prior scoreboard JSON (default: latest in bench/scoreboard/)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="tiny 1-benchmark x 1-backend run to prove the pipeline + schema",
    )
    parser.add_argument(
        "--no-gate",
        action="store_true",
        help="always exit 0 (measure-only; do not fail CI on RED)",
    )
    parser.add_argument(
        "--strict-cold",
        action="store_true",
        help="make WARN_COLD_FLOOR fail the gate too (default: cold-floor warns only)",
    )
    parser.add_argument(
        "--allow-nonauthoritative",
        action="store_true",
        help=(
            "permit a non-authoritative board (origin/main mismatch, dirty tree, "
            "modified tool, or quiescence failure) to "
            "run + not auto-fail the gate via FAIL_STALE — for LOCAL DEBUGGING. "
            "The board is still stamped authoritative=false."
        ),
    )
    parser.add_argument(
        "--pypy",
        nargs="?",
        const="__auto__",
        default=None,
        help="add a PyPy comparator lane (path, or bare flag to auto-detect pypy3.11/3.10)",
    )
    parser.add_argument(
        "--codon",
        nargs="?",
        const="__auto__",
        default=None,
        help="add a Codon AOT comparator lane (path, or bare flag to auto-detect ~/.codon)",
    )
    parser.add_argument(
        "--rebuild-summary",
        default=None,
        help=(
            "re-derive the summary/breakdown/gate from a stored scoreboard's "
            "per-cell data (no re-measurement); writes back in place and "
            "re-applies the gate. Keeps a committed board consistent with the "
            "current tool without rebuilding any binary."
        ),
    )
    parser.add_argument(
        "--merge",
        nargs="+",
        default=None,
        metavar="SRC.json",
        help=(
            "merge per-cell data from multiple scoreboard JSONs into --out "
            "(combine separately-run backend lanes; no re-measurement)"
        ),
    )
    ns = parser.parse_args(argv)

    if ns.rebuild_summary is not None:
        return api["_rebuild_summary"](
            Path(ns.rebuild_summary),
            no_gate=ns.no_gate,
            strict_cold=ns.strict_cold,
            allow_nonauthoritative=ns.allow_nonauthoritative,
        )

    if ns.merge is not None:
        merge_out = (
            Path(ns.out)
            if ns.out
            else api["SCOREBOARD_DIR"] / f"cpython_{api['_git_rev']()}.json"
        )
        return api["_merge_boards"](
            [Path(p) for p in ns.merge],
            merge_out,
            no_gate=ns.no_gate,
            strict_cold=ns.strict_cold,
            allow_nonauthoritative=ns.allow_nonauthoritative,
        )

    backends = ns.backend or ["native", "llvm"]
    profiles = ns.profile or ["release-fast"]

    if ns.self_test:
        ns.set = "smoke"
        ns.benchmark = ["tests/benchmarks/bench_fib.py"]
        backends = ["native"]
        profiles = ["release-fast"]
        ns.samples = max(2, min(ns.samples, 3))
        ns.warmup = 1
        print("[self-test] bench_fib x native x release-fast, samples=%d" % ns.samples)

    scripts = api["_resolve_benchmark_set"](ns.set, ns.benchmark)
    try:
        cpython_oracle = api["_resolve_system_cpython"](ns.cpython)
    except RuntimeError as exc:
        print(f"[scoreboard] {exc}", file=sys.stderr)
        return 2
    cpython_version = cpython_oracle.version
    cpython_identity = cpython_oracle.host_metadata()
    print(
        "[scoreboard] CPython oracle: "
        f"{cpython_oracle.display} "
        f"({cpython_oracle.version}, {cpython_oracle.sys_platform}/"
        f"{cpython_oracle.arch}, {cpython_oracle.pointer_bits}-bit)",
        file=sys.stderr,
    )

    # --- Quiescence guard (#69 Rule 2) — measure BEFORE timing -------------
    # Detect contamination first so a non-quiet machine is stamped
    # authoritative=false (when --require-quiescent) BEFORE any number is taken.
    def emit_quiescence_wait(
        sample: dict, attempt: int, sleep_s: float, remaining_s: float
    ) -> None:
        why = "; ".join(sample.get("reasons", [])) or "machine not quiet"
        print(
            "[scoreboard] waiting for quiescence "
            f"(sample {attempt}, next check in {sleep_s:.0f}s, "
            f"budget left {remaining_s:.0f}s): {why}",
            file=sys.stderr,
        )

    quiescence_wait_s = ns.quiescence_wait_s if ns.require_quiescent else 0.0
    quiescence = api["wait_for_quiescence"](
        timeout_s=quiescence_wait_s,
        poll_s=ns.quiescence_poll_s,
        emit_wait=emit_quiescence_wait,
    )
    if not quiescence["quiet"]:
        print(
            "[scoreboard] machine NOT quiescent — " + "; ".join(quiescence["reasons"]),
            file=sys.stderr,
        )
        if ns.require_quiescent:
            print(
                "[scoreboard] *** NON-AUTHORITATIVE: machine not quiet; do not "
                "optimize from this red list (EXPLORATORY only) ***",
                file=sys.stderr,
            )
    else:
        waited_s = float(quiescence.get("quiescence_waited_s") or 0.0)
        waited_note = f" after waiting {waited_s:.0f}s" if waited_s else ""
        print(
            f"[scoreboard] machine quiescent{waited_note} "
            f"(load={quiescence['loadavg_1m']} "
            f"ncpu={quiescence['ncpu']} runnable={quiescence['runnable_signal']} "
            f"builds=0)",
            file=sys.stderr,
        )

    # --- #76 WARM-HOT cycle attribution path (looped + symbolicated) -------
    # A self-contained profiling path: for each benchmark build a looped +
    # symbolicated variant, sample its steady state, apply the refusal gate, and
    # write a JSON profile (the cycle facts). It is NOT a speedup measurement and
    # never runs the release gate — so it returns here before the timing sweep.
    if ns.sample_hot_only:
        if len(backends) != 1:
            print(
                "[hot-only] one backend per run; pass exactly one --backend "
                f"(got {backends}) — defaulting to {backends[0]}",
                file=sys.stderr,
            )
        spec = api["BACKENDS_BY_NAME"][backends[0]]
        profile = profiles[0]
        if ns.inner_repeat < 2:
            print(
                f"[hot-only] --inner-repeat={ns.inner_repeat} < 2 (nothing to "
                "amortize); refusing.",
                file=sys.stderr,
            )
            return 2
        hot_cells = api["run_hot_only_profiles"](
            scripts=scripts,
            spec=spec,
            profile=profile,
            inner_loops=ns.inner_repeat,
            rss_mb=ns.rss_mb,
        )
        return api["_emit_hot_only_board"](
            hot_cells,
            spec=spec,
            profile=profile,
            inner_loops=ns.inner_repeat,
            quiescence=quiescence,
            cpython_version=cpython_version,
            out=ns.out,
        )

    # --- Provenance + authoritative gate (council ruling A + B + #69) ------
    specs_profiles = [
        (api["BACKENDS_BY_NAME"][b], p) for b in backends for p in profiles
    ]
    provenance = api["gather_provenance"](
        specs_profiles,
        quiescence=quiescence,
        require_quiescent=ns.require_quiescent,
    )
    # provenance.authoritative records the TRUTH (tree==origin, clean, tool
    # unmodified). `--allow-nonauthoritative` does NOT change that truth — it
    # lets the cells classify on their REAL numbers (not FAIL_STALE) for local
    # debugging, while the board still records authoritative=false and the gate
    # is told not to auto-fail on staleness.
    authoritative = bool(provenance.get("authoritative", True))
    effective_authoritative = authoritative or ns.allow_nonauthoritative
    if not authoritative:
        print(
            "[scoreboard] *** WARNING: scoreboard provenance is non-authoritative; "
            "benchmark is exploratory unless explicitly requested ***",
            file=sys.stderr,
        )
        print(
            f"[scoreboard]     reason: {provenance.get('authoritative_reason')}",
            file=sys.stderr,
        )
        if ns.allow_nonauthoritative:
            print(
                "[scoreboard]     --allow-nonauthoritative: classifying real "
                "numbers; board stays authoritative=false; gate will NOT "
                "FAIL_STALE.",
                file=sys.stderr,
            )
        else:
            print(
                "[scoreboard]     (the gate will FAIL_STALE; pass "
                "--allow-nonauthoritative to run for local debugging)",
                file=sys.stderr,
            )
    if api["_refuses_nonauthoritative_measurement"](
        authoritative=authoritative,
        allow_nonauthoritative=ns.allow_nonauthoritative,
    ):
        print(
            "[scoreboard] refusing non-authoritative measurement before "
            "starting benchmark builds",
            file=sys.stderr,
        )
        return 1

    # --- PyPy / Codon comparator lanes (council Lane C) --------------------
    pypy_bin = api["_resolve_pypy"](ns.pypy) if ns.pypy is not None else None
    pypy_version = api["_probe_interp_version"](pypy_bin) if pypy_bin else None
    if pypy_bin:
        print(
            f"[scoreboard] PyPy comparator: {pypy_bin} ({pypy_version})",
            file=sys.stderr,
        )
    codon_bin = api["_resolve_codon"](ns.codon) if ns.codon is not None else None
    codon_runner = api["CodonRunner"](codon_bin) if codon_bin else None
    codon_version = api["_probe_codon_version"](codon_bin) if codon_bin else None
    if codon_bin:
        print(
            f"[scoreboard] Codon comparator: {codon_bin} ({codon_version})",
            file=sys.stderr,
        )

    budgets = api["_load_cold_start_budgets"]()

    git_rev = api["_git_rev"]()
    api["SCOREBOARD_DIR"].mkdir(parents=True, exist_ok=True)
    out_path = (
        Path(ns.out) if ns.out else api["SCOREBOARD_DIR"] / f"cpython_{git_rev}.json"
    )
    log_dir = api["SCOREBOARD_DIR"] / f"logs_{git_rev}"
    partial_path = out_path.with_suffix(".partial.json")

    benchmarks_run: list[str] = []
    benchmarks_deferred: list[dict] = []
    cells: list[Cell] = []

    # Per (backend, profile) we open ONE daemon batch build server and reuse it
    # across the whole benchmark set — matching bench.py's amortized-build model.
    for backend_name in backends:
        spec = api["BACKENDS_BY_NAME"][backend_name]
        for profile in profiles:
            cell_budget_ms = api["_budget_ms_for"](budgets, backend_name, profile)
            batch_server = None
            if spec.build_target == "native":
                try:
                    batch_server = api["bench"]._BenchBatchBuildServer(
                        api["_perfscore_build_env"](spec)
                    )
                except Exception as exc:  # noqa: BLE001
                    print(
                        f"[warn] could not start batch build server for "
                        f"{backend_name}/{profile}: {exc!r}; falling back to per-build",
                        file=sys.stderr,
                    )
                    batch_server = None
            try:
                for script in scripts:
                    key = api["bench_suites"].canonical_benchmark_key(script)
                    print(
                        f"[scoreboard] {key} | {backend_name} | {profile} ...",
                        file=sys.stderr,
                        flush=True,
                    )
                    cell = api["measure_cell"](
                        script_path=script,
                        spec=spec,
                        profile=profile,
                        samples=ns.samples,
                        warmup=ns.warmup,
                        rss_mb=ns.rss_mb,
                        timeout_s=ns.timeout,
                        batch_server=batch_server,
                        cpython_cmd=cpython_oracle.cmd,
                        log_dir=log_dir,
                        budget_ms=cell_budget_ms,
                        authoritative=effective_authoritative,
                        pypy_bin=pypy_bin,
                        codon_runner=codon_runner,
                        repeat=ns.repeat,
                        emit_cycle_profile=ns.emit_cycle_profile,
                    )
                    cells.append(cell)
                    if key not in benchmarks_run:
                        benchmarks_run.append(key)
                    # CPython-incompatible benchmarks have no valid floor and
                    # are excluded from the gate — record the exclusion
                    # explicitly (no silent truncation).
                    if cell.verdict == api["VERDICT_CPY_INCOMPAT"]:
                        dkey = f"{key} [{backend_name}/{profile}]"
                        if not any(d["benchmark"] == dkey for d in benchmarks_deferred):
                            benchmarks_deferred.append(
                                {
                                    "benchmark": dkey,
                                    "reason": cell.note
                                    or "CPython baseline could not run this script",
                                }
                            )
                    # Checkpoint partial JSON after every cell (death-recoverable).
                    try:
                        api["_checkpoint"](
                            partial_path,
                            cells,
                            benchmarks_run,
                            benchmarks_deferred,
                            cpython_version,
                            ns.samples,
                            ns.warmup,
                            provenance=provenance,
                            cpython_identity=cpython_identity,
                            pypy_version=pypy_version,
                            codon_version=codon_version,
                        )
                    except api["ScoreboardSchemaError"] as exc:
                        api["_print_schema_error"](exc)
                        return 3
                    print(
                        f"    -> {cell.verdict}  warm={api['_fmt'](cell.warm_speedup)} "
                        f"cold={api['_fmt'](cell.cold_speedup)} tax={api['_fmt'](cell.startup_tax_ms, 0)}ms",
                        file=sys.stderr,
                        flush=True,
                    )
            finally:
                if batch_server is not None:
                    try:
                        batch_server.close()
                    except Exception:  # noqa: BLE001
                        pass
    if codon_runner is not None:
        codon_runner.close()

    # --- 5-state classification (#69 --classify) ---------------------------
    # Set after the whole sweep so the WHOLE-board quiescence + each cell's
    # repeat CI are both available. DIMENSIONAL_WIN needs the baseline board.
    if ns.classify:
        baseline_doc = None
        if ns.baseline is not None:
            bpath = (
                api["_latest_baseline"](exclude=out_path)
                if ns.baseline == "__latest__"
                else Path(ns.baseline)
            )
            if bpath is not None and bpath.exists():
                try:
                    baseline_doc = json.loads(bpath.read_text(encoding="utf-8"))
                except (OSError, json.JSONDecodeError):
                    baseline_doc = None
        api["apply_classification"](
            cells, quiescent=bool(quiescence["quiet"]), baseline_doc=baseline_doc
        )

    doc = api["build_scoreboard_doc"](
        cells,
        benchmarks_run=benchmarks_run,
        benchmarks_deferred=benchmarks_deferred,
        cpython_version=cpython_version,
        samples=ns.samples,
        warmup=ns.warmup,
        provenance=provenance,
        cpython_identity=cpython_identity,
        pypy_version=pypy_version,
        codon_version=codon_version,
    )
    if ns.print_provenance:
        api["_print_provenance"](provenance)
    # Attach the regressions-from-last-green list so print_summary can surface
    # it in the classified output (council ruling A section).
    doc["_out_path"] = str(out_path)
    api["_attach_regressions"](doc)
    doc.pop("_out_path", None)
    try:
        api["_write_scoreboard_doc"](out_path, doc, context=f"scoreboard {out_path}")
    except api["ScoreboardSchemaError"] as exc:
        api["_print_schema_error"](exc)
        return 3
    if partial_path.exists():
        partial_path.unlink()
    print(f"\nscoreboard JSON -> {out_path}", file=sys.stderr)

    if ns.self_test:
        # The self-test PROVES the pipeline + schema, not the perf/stale gate.
        # It inherently dirties the tree (the tool under test is modified), so
        # subjecting it to FAIL_STALE would be circular — it validates the
        # SCHEMA and returns on that alone.
        problems = api["validate_board"](doc)
        api["print_summary"](doc)
        if problems:
            print("[self-test] SCHEMA VALIDATION FAILED:", file=sys.stderr)
            for p in problems:
                print(f"    - {p}", file=sys.stderr)
            return 3
        print(
            "[self-test] schema OK: required top-level keys + per-cell fields present, "
            "2-D verdict + provenance + gate wired, JSON round-trips.",
            file=sys.stderr,
        )
        return 0

    api["print_summary"](doc)

    if ns.baseline is not None:
        baseline_path = (
            api["_latest_baseline"](exclude=out_path)
            if ns.baseline == "__latest__"
            else Path(ns.baseline)
        )
        if baseline_path is None or not baseline_path.exists():
            print("[baseline] no prior scoreboard to diff against.", file=sys.stderr)
        else:
            newly_red, regressed = api["diff_against_baseline"](doc, baseline_path)
            print(f"\n[baseline diff vs {baseline_path.name}]")
            if newly_red:
                print("  NEWLY GATING:")
                for m in newly_red:
                    print(f"    {m}")
            if regressed:
                print("  REGRESSED (still passing):")
                for m in regressed:
                    print(f"    {m}")
            if not newly_red and not regressed:
                print("  no new reds, no regressions.")

    return api["_gate_exit_code"](
        doc,
        no_gate=ns.no_gate,
        strict_cold=ns.strict_cold,
        allow_nonauthoritative=ns.allow_nonauthoritative,
    )
