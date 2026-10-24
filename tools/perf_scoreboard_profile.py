#!/usr/bin/env python3
from __future__ import annotations

import datetime as dt
import json
import sys
import subprocess
from pathlib import Path

from molt.process_environment import ProcessEnvironment
from tools import harness_memory_guard
from tools.sample_profile import read_self_time
from tools import bench
from tools import bench_suites
from tools import perf_inner_repeat
from tools.perf_schema import SCHEMA_VERSION
from tools.perf_scoreboard_model import (
    REPO_ROOT,
    SAFE_RUN,
    SCOREBOARD_DIR,
    BackendSpec,
    RunOutcome,
)


def _facade():
    from tools import perf_scoreboard as ps

    return ps


def _resolve_sampler():
    return _facade()._resolve_sampler()


def _profiling_popen(*args, **kwargs):
    return _facade()._profiling_popen(*args, **kwargs)


def _perfscore_build_env(spec: BackendSpec, profile: str) -> ProcessEnvironment:
    return _facade()._perfscore_build_env(spec, profile)


def _release_binary(binary, primary):
    return _facade()._release_binary(binary, primary)


def _git_rev():
    return _facade()._git_rev()


def _run_measured_command(*args, **kwargs):
    return _facade()._run_measured_command(*args, **kwargs)


def _shquote(arg: str) -> str:
    """Minimal POSIX shell quote for embedding an argv element in `sh -c`."""
    import shlex

    return shlex.quote(arg)


def _terminate(proc: subprocess.Popen) -> None:
    harness_memory_guard.force_close_process_group(proc)


def _is_launch_frame(symbol: str, lib: str | None) -> bool:
    """True iff a leaf frame is process launch / first-touch page-in, not work.

    ``_dyld_start`` (in dyld) is the dynamic-loader entry: it covers process
    launch AND the first-touch page-in of the static binary's text. That is the
    cost inner-repeat exists to amortize; if it still dominates, the loop factor
    was too small (the refusal gate fires).
    """
    sym = (symbol or "").lstrip("_")
    for ls, ll in _LAUNCH_FRAMES:
        if sym == ls.lstrip("_") and (lib or "") == ll:
            return True
    return False


def classify_launch_dominance(top_symbols: list[dict]) -> dict:
    """Compute the launch/page-in vs in-binary breakdown of a sample leaderboard.

    Returns ``{total, launch_samples, launch_fraction, in_binary_samples,
    in_binary_fraction, launch_dominates}``. ``launch_dominates`` is the
    refusal signal: True iff launch/page-in is >= the refusal fraction of the
    whole leaf-self-time leaderboard, meaning the steady-state hot path is NOT
    yet legible and a hot-path claim must be refused.
    """
    total = sum(int(s.get("self_samples", 0)) for s in top_symbols)
    if total <= 0:
        return {
            "total": 0,
            "launch_samples": 0,
            "launch_fraction": None,
            "in_binary_samples": 0,
            "in_binary_fraction": None,
            "launch_dominates": True,  # no signal -> cannot attribute -> refuse
        }
    launch = sum(
        int(s.get("self_samples", 0))
        for s in top_symbols
        if _is_launch_frame(s.get("symbol", ""), s.get("lib"))
    )
    launch_frac = launch / total
    return {
        "total": total,
        "launch_samples": launch,
        "launch_fraction": round(launch_frac, 4),
        "in_binary_samples": total - launch,
        "in_binary_fraction": round((total - launch) / total, 4),
        "launch_dominates": launch_frac >= LAUNCH_DOMINANCE_REFUSAL_FRACTION,
    }


def top_in_binary_frames(
    top_symbols: list[dict], *, binary_lib: str | None, top_n: int = 20
) -> list[dict]:
    """The heaviest IN-BINARY (molt user/runtime) frames — the cycle facts.

    Filters the leaderboard to frames whose ``lib`` is the profiled binary (so
    libsystem/dyld helpers are excluded) and annotates each with its share of
    the WHOLE leaderboard (``leaderboard_pct``), which is the attribution unit.
    """
    total = sum(int(s.get("self_samples", 0)) for s in top_symbols) or 1
    out: list[dict] = []
    for s in top_symbols:
        lib = s.get("lib")
        if binary_lib is not None and lib != binary_lib:
            continue
        if s.get("symbol") == "???":
            # An unsymbolicated in-binary frame: record it (with its offset text
            # if the parser preserved it) but it is not yet a named cycle fact.
            pass
        out.append(
            {
                "symbol": s.get("symbol"),
                "self_samples": int(s.get("self_samples", 0)),
                "leaderboard_pct": round(
                    100.0 * int(s.get("self_samples", 0)) / total, 2
                ),
                "lib": lib,
            }
        )
        if len(out) >= top_n:
            break
    return out


def _profiling_tmp_root(env: ProcessEnvironment) -> Path:
    """Profiling scratch follows the build's admitted artifact root."""
    return bench._bench_tmp_root(env)


MOLT_KEEP_SYMBOLS_ENV = "MOLT_KEEP_SYMBOLS"

DEFAULT_INNER_REPEAT = 40

HOT_SAMPLE_WARMUP_S = 0.6

HOT_SAMPLE_WINDOW_S = 3.0

LAUNCH_DOMINANCE_REFUSAL_FRACTION = 0.40

_LAUNCH_FRAMES = (
    ("_dyld_start", "dyld"),
    ("__dyld_start", "dyld"),
)


def build_profiling_binary(
    script_path: Path,
    *,
    spec: "BackendSpec",
    profile: str,
    target_python_version: str,
    inner_loops: int,
    log_lines: list[str],
) -> "tuple[bench.MoltBinary | None, dict]":
    """Build the LOOPED + SYMBOLICATED profiling variant of a benchmark.

    Two transforms vs the normal cell build:
      1. INNER-REPEAT — the benchmark's ``main()`` is wrapped in
         ``for _ in range(N): main()`` (``perf_inner_repeat``) so launch/page-in
         amortizes inside one process. Refuses (and returns the reason) if the
         benchmark is not the semantics-preservingly loopable shape.
      2. SYMBOLICATE — built with ``MOLT_KEEP_SYMBOLS=1`` so the final link
         keeps molt user-fn / runtime symbol names.

    Returns ``(binary_or_None, meta)`` where ``meta`` documents the transform
    (``inner_loops``, ``symbolicated``, ``looped``, ``refused``/``reason``).
    This binary is for CYCLE ATTRIBUTION ONLY — never for the speedup number
    (the timing path measures the shipped, stripped one-shot binary).
    """
    meta: dict = {
        "inner_loops": inner_loops,
        "symbolicated": True,
        "looped": False,
        "refused": False,
        "reason": None,
        "looped_source_path": None,
        "target_python": target_python_version,
    }
    from molt.target_python_policy import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS

    if target_python_version not in SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS:
        meta["refused"] = True
        meta["reason"] = "profiling requires an explicit supported target Python minor"
        return None, meta
    if spec.build_target != "native":
        meta["refused"] = True
        meta["reason"] = (
            "hot-only cycle attribution requires the native sampler backend"
        )
        return None, meta
    try:
        source = script_path.read_text(encoding="utf-8")
    except OSError as exc:
        meta["refused"] = True
        meta["reason"] = f"could not read benchmark source: {exc!r}"
        return None, meta

    plan = perf_inner_repeat.analyze(source, inner_loops=inner_loops)
    if not plan.ok:
        meta["refused"] = True
        meta["reason"] = f"inner-repeat refused: {plan.reason}"
        log_lines.append(f"PROFILING-BUILD REFUSED (inner-repeat): {plan.reason}")
        return None, meta
    meta["looped"] = True

    build_env = _perfscore_build_env(spec, profile).updated(
        {MOLT_KEEP_SYMBOLS_ENV: "1"}
    )
    from tools.perf_scoreboard_build_profiles import profile_selection

    selection = profile_selection(spec, profile)
    source_owner = bench._new_bench_temp(_profiling_tmp_root(build_env), build_env)
    binary = None
    primary: BaseException | None = None
    try:
        looped_path = Path(source_owner.name) / script_path.name
        looped_path.write_text(plan.source, encoding="utf-8")
        meta["looped_source_path"] = str(looped_path)
        extra_args = [
            "--python-version",
            target_python_version,
            "--backend",
            selection.codegen_backend,
            *bench_suites.molt_args_for_benchmark(script_path),
        ]
        binary = bench.prepare_molt_binary(
            str(looped_path),
            extra_args=extra_args,
            env=build_env,
            build_profile=selection.guest_profile,
            batch_server=None,  # symbolicated env differs from the cell server's
            build_timeout_s=600.0,
        )
        if not isinstance(binary, bench.MoltBinary):
            meta["refused"] = True
            if isinstance(binary, bench.MoltFailure):
                meta["reason"] = f"profiling build failed: {binary.status}"
                detail = f" detail={binary.detail}" if binary.detail else ""
                log_lines.append(
                    f"PROFILING-BUILD FAILED status={binary.status}{detail}"
                )
            else:
                meta["reason"] = "profiling build produced no binary"
                log_lines.append("PROFILING-BUILD FAILED")
            return None, meta
        log_lines.append(
            f"profiling binary built: looped(inner_loops={inner_loops}) + symbolicated "
            f"size_kib={round(binary.size_kb, 1)}"
        )
        return binary, meta
    except BaseException as exc:
        primary = exc
        raise
    finally:
        # The compiler has consumed the variant. A failed source retirement
        # prevents transferring a successful binary without its ownership context.
        try:
            bench._cleanup_bench_temp(source_owner, primary)
        except BaseException as exc:
            if isinstance(binary, bench.MoltBinary):
                _release_binary(binary, exc)
            elif isinstance(binary, bench.MoltFailure):
                exc.benchmark_build_failure = binary
                exc.add_note(f"profiling build failed before cleanup: {binary.status}")
            raise
        else:
            if primary is not None and isinstance(binary, bench.MoltBinary):
                _release_binary(binary, primary)


def _time_one_run(
    cmd: list[str], *, env: ProcessEnvironment, rss_mb: int, timeout_s: float
) -> "RunOutcome":
    """Wall-time ONE run of ``cmd`` under safe_run (for sizing the sample window).

    Returns the full RunOutcome so the caller can distinguish an OOM (an
    inner-repeat that amplifies a per-iteration leak past the RSS cap — a real
    finding) from a generic run failure.
    """
    return _run_measured_command(
        cmd,
        env=env,
        rss_mb=rss_mb,
        timeout_s=timeout_s,
        label="hot-size",
    )


def capture_hot_only_profile(
    binary_path: Path,
    *,
    run_args: list[str],
    env: ProcessEnvironment,
    rss_mb: int,
    inner_loops: int,
    warmup_s: float = HOT_SAMPLE_WARMUP_S,
    window_s: float = HOT_SAMPLE_WINDOW_S,
    top_n: int = 30,
) -> dict:
    """Sample a LOOPED+SYMBOLICATED process in STEADY STATE (#76).

    Three steps, all in ONE process per phase so launch/page-in is paid once and
    the steady-state hot path dominates:

      1. SIZE — time one looped run to learn its lifetime ``T``. The inner-repeat
         must keep the process alive for at least ``warmup_s + a short window``;
         if ``T`` is too short, the loop factor is too small to carve out a
         steady-state window and we REFUSE ("increase --inner-repeat") — never
         sample a process that exits mid-window.
      2. WARMUP — launch the looped process, sleep ``warmup_s`` so the first
         iterations (cold I-cache, first-touch page-in) are NOT in the window,
         then attach ``/usr/bin/sample`` to the already-running process (no
         ``-wait`` needed; ``/usr/bin/sample`` has no built-in warmup delay, so
         the warmup is realized by delaying the attach).
      3. SAMPLE — accumulate leaf self-time for ``window`` seconds of steady
         state, fitted to the remaining lifetime so it closes before exit.

    Applies the REFUSAL rule: if launch/page-in still accounts for >= the
    refusal fraction of leaf self-time, the loop factor was too small — returns
    ``available=False`` with ``refused_reason`` and NO hot-path claim (fail
    closed, same as #69's quiescence guard). Otherwise returns the in-binary hot
    frames (the cycle facts that select the next optimization).
    """
    import time as _time

    sampler = _resolve_sampler()
    if sampler is None:
        return {
            "available": False,
            "mode": "hot-only",
            "top_symbols": [],
            "in_binary_top": [],
            "launch_breakdown": None,
            "refused": True,
            "refused_reason": "cycle profiler unavailable (/usr/bin/sample not found)",
            "note": "sampler unavailable",
        }
    target_name = binary_path.name
    cmd = [str(binary_path), *run_args]

    # --- (1) SIZE: time one looped run so we can fit the steady-state window ---
    size = _time_one_run(
        cmd, env=env, rss_mb=rss_mb, timeout_s=warmup_s + window_s + 120
    )
    if not size.ok:
        if size.status == "oom":
            reason = (
                f"looped(inner_loops={inner_loops}) binary exceeded the {rss_mb} MiB "
                "RSS cap — the inner-repeat amplified a per-iteration molt LEAK "
                "(each main() call leaks its working set; a one-shot run hides it). "
                "LOWER --inner-repeat to profile a bounded window; the leak itself "
                "is a separate compiler-RC finding"
            )
            note = "size run OOM (inner-repeat amplified a per-iteration leak)"
        else:
            reason = (
                f"looped profiling binary failed to run (size phase: {size.status})"
            )
            note = "size run failed"
        return {
            "available": False,
            "mode": "hot-only",
            "top_symbols": [],
            "in_binary_top": [],
            "launch_breakdown": None,
            "leak_suspected": size.status == "oom",
            "refused": True,
            "refused_reason": reason,
            "note": note,
            "size_status": size.status,
            "size_exit_code": size.exit_code,
            "size_stdout_tail": size.stdout_tail,
            "size_stderr_tail": size.stderr_tail,
        }
    looped_runtime_s = size.elapsed_s or 0.0
    # The steady window we can actually carve out after warmup, leaving a 0.3s
    # tail so the sampler closes before the process exits. Need a real window.
    min_window_s = 0.8
    avail_window_s = looped_runtime_s - warmup_s - 0.3
    if avail_window_s < min_window_s:
        return {
            "available": False,
            "mode": "hot-only",
            "top_symbols": [],
            "in_binary_top": [],
            "launch_breakdown": None,
            "refused": True,
            "refused_reason": (
                f"CYCLE-ATTRIBUTION INVALID: looped runtime {looped_runtime_s:.2f}s "
                f"is too short to carve a steady-state window after {warmup_s:.1f}s "
                f"warmup (need >= {warmup_s + 0.3 + min_window_s:.1f}s) — "
                "increase --inner-repeat"
            ),
            "note": "looped runtime too short for a steady window",
        }
    eff_window_s = max(min_window_s, min(window_s, avail_window_s))

    output_owner = bench._new_bench_temp(bench._bench_tmp_root(env), env)
    primary: BaseException | None = None
    sampler_proc = None
    proc = None
    try:
        out_file = Path(output_owner.name) / "sample.txt"
        quoted = " ".join(_shquote(a) for a in cmd)
        run_one = f"{quoted} >/dev/null 2>&1 || true"
        safe_cmd = [
            sys.executable,
            str(SAFE_RUN),
            "--rss-mb",
            str(rss_mb),
            "--timeout",
            str(int(warmup_s + eff_window_s + 60)),
            "--",
            "/bin/sh",
            "-c",
            run_one,
        ]
        # --- (2) WARMUP: launch the workload, sleep warmup_s, THEN attach ---------
        try:
            proc = _profiling_popen(safe_cmd, env=env)
        except OSError as exc:
            primary = exc
            return {
                "available": False,
                "mode": "hot-only",
                "top_symbols": [],
                "in_binary_top": [],
                "launch_breakdown": None,
                "refused": True,
                "refused_reason": f"could not launch profiling target: {exc!r}",
                "note": "target launch failed",
            }
        _time.sleep(warmup_s)  # let the first iterations warm up (excluded from window)
        # --- (3) SAMPLE: attach to the now-running steady-state process -----------
        try:
            sampler_proc = _profiling_popen(
                [
                    sampler,
                    target_name,
                    str(max(1, int(round(eff_window_s)))),
                    "-mayDie",
                    "-f",
                    str(out_file),
                ]
            )
        except OSError as exc:
            primary = exc
            return {
                "available": False,
                "mode": "hot-only",
                "top_symbols": [],
                "in_binary_top": [],
                "launch_breakdown": None,
                "refused": True,
                "refused_reason": f"could not start sampler: {exc!r}",
                "note": "sampler start failed",
            }
        try:
            sampler_proc.wait(timeout=eff_window_s + 40)
        except subprocess.TimeoutExpired:
            _terminate(sampler_proc)
        sampler_proc = None
        _terminate(proc)
        proc = None
        symbols = read_self_time(out_file, top_n=max(top_n, 60))
        if not symbols:
            return {
                "available": False,
                "mode": "hot-only",
                "top_symbols": [],
                "in_binary_top": [],
                "launch_breakdown": None,
                "refused": True,
                "refused_reason": (
                    "sampler produced no parseable symbols even after fitting the "
                    "window to the looped runtime — raise --inner-repeat so the "
                    "steady-state window is longer"
                ),
                "note": "no samples",
            }
        breakdown = classify_launch_dominance(symbols)
        note = (
            f"/usr/bin/sample {eff_window_s:.1f}s steady-state (after {warmup_s:.1f}s "
            f"warmup) of ONE looped(inner_loops={inner_loops}) + symbolicated process; "
            f"looped runtime {looped_runtime_s:.2f}s — CYCLES"
        )
        if breakdown["launch_dominates"]:
            lf = breakdown["launch_fraction"]
            lf_pct = f"{100 * lf:.1f}%" if lf is not None else "n/a"
            return {
                "available": False,
                "mode": "hot-only",
                "top_symbols": symbols[:top_n],
                "in_binary_top": [],
                "launch_breakdown": breakdown,
                "refused": True,
                "refused_reason": (
                    f"CYCLE-ATTRIBUTION INVALID: launch/page-in dominates leaf "
                    f"self-time ({lf_pct} >= "
                    f"{int(100 * LAUNCH_DOMINANCE_REFUSAL_FRACTION)}%) even after "
                    f"inner-repeat — increase --inner-repeat"
                ),
                "note": note,
            }
        in_binary_top = top_in_binary_frames(
            symbols, binary_lib=target_name, top_n=top_n
        )
        return {
            "available": True,
            "mode": "hot-only",
            "inner_loops": inner_loops,
            "top_symbols": symbols[:top_n],
            "in_binary_top": in_binary_top,
            "launch_breakdown": breakdown,
            "refused": False,
            "refused_reason": None,
            "note": note,
        }
    except BaseException as exc:
        primary = exc
        raise
    finally:
        try:
            try:
                if sampler_proc is not None:
                    _terminate(sampler_proc)
            finally:
                if proc is not None:
                    _terminate(proc)
        except BaseException as exc:
            if primary is not None:
                primary.add_note(f"profiling process cleanup failed: {exc!r}")
                raise primary
            primary = exc
            raise
        finally:
            bench._cleanup_bench_temp(output_owner, primary)


def run_hot_only_profiles(
    *,
    scripts: list[Path],
    spec: "BackendSpec",
    profile: str,
    target_python_version: str,
    inner_loops: int,
    rss_mb: int,
    warmup_s: float = HOT_SAMPLE_WARMUP_S,
    window_s: float = HOT_SAMPLE_WINDOW_S,
    top_n: int = 30,
) -> list[dict]:
    """Drive the #76 hot-only profiler for each benchmark and return per-bench cells.

    For each benchmark: build the LOOPED + SYMBOLICATED variant, sample its
    steady state, apply the refusal gate, and collect the in-binary hot frames.
    Each returned cell is a self-contained attribution record (the cycle fact),
    NOT a scoreboard speedup cell — these never touch the gate.
    """
    results: list[dict] = []
    for script in scripts:
        key = bench_suites.canonical_benchmark_key(script)
        log_lines: list[str] = [f"# HOT-ONLY {key} | {spec.backend} | {profile}"]
        print(
            f"[hot-only] {key} | inner_loops={inner_loops} | symbolicated ...",
            file=sys.stderr,
            flush=True,
        )
        binary, build_meta = build_profiling_binary(
            script,
            spec=spec,
            profile=profile,
            target_python_version=target_python_version,
            inner_loops=inner_loops,
            log_lines=log_lines,
        )
        cell: dict = {
            "benchmark": key,
            "target": spec.target,
            "backend": spec.backend,
            "profile": profile,
            "target_python": target_python_version,
            "inner_loops": inner_loops,
            "build": build_meta,
        }
        if binary is None:
            cell["profile_result"] = {
                "available": False,
                "refused": True,
                "refused_reason": build_meta.get("reason"),
            }
            results.append(cell)
            print(
                f"    -> REFUSED ({build_meta.get('reason')})",
                file=sys.stderr,
                flush=True,
            )
            continue
        primary: BaseException | None = None
        try:
            run_args = bench.resolve_benchmark_run_args(str(script))
            prof = capture_hot_only_profile(
                Path(binary.path),
                run_args=run_args,
                env=_perfscore_build_env(spec, profile),
                rss_mb=rss_mb,
                inner_loops=inner_loops,
                warmup_s=warmup_s,
                window_s=window_s,
                top_n=top_n,
            )
        except BaseException as exc:
            primary = exc
            raise
        finally:
            _release_binary(binary, primary)
        cell["profile_result"] = prof
        results.append(cell)
        if prof.get("refused"):
            print(
                f"    -> REFUSED ({prof.get('refused_reason')})",
                file=sys.stderr,
                flush=True,
            )
        else:
            bd = prof.get("launch_breakdown") or {}
            lf = bd.get("launch_fraction")
            top = prof.get("in_binary_top") or []
            head = top[0]["symbol"] if top else "-"
            print(
                f"    -> HOT (launch={100 * lf:.1f}% < "
                f"{int(100 * LAUNCH_DOMINANCE_REFUSAL_FRACTION)}%) "
                f"top: {head}",
                file=sys.stderr,
                flush=True,
            )
    return results


def _emit_hot_only_board(
    hot_cells: list[dict],
    *,
    spec: "BackendSpec",
    profile: str,
    inner_loops: int,
    quiescence: dict,
    cpython_version: str,
    out: str | None,
) -> int:
    """Write the #76 hot-only profile board (JSON) + print the attribution report.

    Returns 0 when every requested benchmark produced a hot-path attribution,
    1 when ANY was REFUSED (launch still dominated, or the build/transform was
    refused) — the fail-closed signal that the loop factor or shape needs work.
    """
    git_rev = _git_rev()
    SCOREBOARD_DIR.mkdir(parents=True, exist_ok=True)
    out_path = (
        Path(out)
        if out
        else SCOREBOARD_DIR / f"hot_profile_{spec.backend}_{git_rev}.json"
    )
    doc = {
        "schema_version": SCHEMA_VERSION,
        "kind": "hot_only_cycle_profile",
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": git_rev,
        "backend": spec.backend,
        "target": spec.target,
        "profile": profile,
        "inner_loops": inner_loops,
        "symbolicated": True,
        "symbolicate_mechanism": f"{MOLT_KEEP_SYMBOLS_ENV}=1 (link-strip + post-link strip skipped)",
        "launch_refusal_fraction": LAUNCH_DOMINANCE_REFUSAL_FRACTION,
        "cpython_baseline": cpython_version,
        "execution_host": None,
        "quiescence": quiescence,
        "methodology": (
            "Inner-repeat the benchmark main() N times in ONE process so launch/"
            "page-in (_dyld_start) amortizes; build with MOLT_KEEP_SYMBOLS=1 so the "
            "linker keeps molt user-fn/runtime symbols; /usr/bin/sample the steady "
            "state after a warmup delay. REFUSE a hot-path claim if launch/page-in "
            f">= {int(100 * LAUNCH_DOMINANCE_REFUSAL_FRACTION)}% of leaf self-time."
        ),
        "cells": hot_cells,
    }
    out_path.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")

    # --- Report ----------------------------------------------------------
    print("\n" + "=" * 72)
    print(f"WARM-HOT CYCLE ATTRIBUTION (#76) — {spec.backend}/{profile}")
    print(f"  inner_loops={inner_loops}  symbolicated={MOLT_KEEP_SYMBOLS_ENV}=1")
    print("=" * 72)
    any_refused = False
    for cell in hot_cells:
        pr = cell.get("profile_result", {})
        key = cell["benchmark"]
        if pr.get("refused") or not pr.get("available"):
            any_refused = True
            print(f"\n  {key}")
            print(f"    REFUSED: {pr.get('refused_reason')}")
            bd = pr.get("launch_breakdown")
            if bd and bd.get("launch_fraction") is not None:
                print(
                    f"    (launch/page-in = {100 * bd['launch_fraction']:.1f}% of "
                    f"{bd['total']} leaf samples)"
                )
            continue
        bd = pr.get("launch_breakdown", {})
        lf = bd.get("launch_fraction")
        ibf = bd.get("in_binary_fraction")
        print(f"\n  {key}  [HOT — attribution VALID]")
        print(
            f"    launch/page-in: {100 * lf:.1f}%   in-binary: {100 * ibf:.1f}%   "
            f"({bd['total']} leaf samples)"
        )
        print("    TOP IN-BINARY HOT FRAMES (the cycle facts):")
        for s in (pr.get("in_binary_top") or [])[:12]:
            print(f"      {s['leaderboard_pct']:5.1f}%  {s['symbol']}")
    try:
        shown = out_path.relative_to(REPO_ROOT)
    except ValueError:
        shown = out_path  # --out outside the repo (e.g. /tmp): show absolute
    print(f"\n  -> wrote {shown}")
    if any_refused:
        print(
            "  -> SOME benchmarks REFUSED (launch dominated / not loopable): "
            "raise --inner-repeat or inspect the reason.",
        )
        return 1
    print("  -> all benchmarks attributed to in-binary hot frames.")
    return 0
