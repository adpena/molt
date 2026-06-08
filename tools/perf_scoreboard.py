#!/usr/bin/env python3
"""CPython floor-scoreboard — the release-blocking performance gate.

Operationalizes the molt **Performance Constitution** (CLAUDE.md, commit
538f4386e): CPython is the absolute floor. Any benchmark whose molt SPEEDUP
(``cpython_time / molt_time``) is below ``1.00`` is **RED** — a contract
violation, not "later optimization work."

This tool MEASURES and SURFACES; it does not fix slow benchmarks. It emits a
single machine-readable scoreboard keyed ``benchmark x target x backend x
profile`` reporting the absolute molt/CPython speedup, binary size, peak RSS,
compile time, and cold + warm ratios with a stability flag, and exits nonzero
if ANY cell is RED or unstable-unmeasurable (CI-gateable).

Direction (LABELLED unambiguously): ``speedup = cpython_time / molt_time``.
  * ``speedup > 1.0``  -> molt is FASTER than CPython (GREEN).
  * ``speedup < 1.0``  -> molt is SLOWER than CPython (RED — contract violation).

Reuse posture
-------------
The canonical molt-vs-CPython wall-time machinery already lives in
``tools/bench.py`` (daemon batch build with the harness memory guard,
binary-size + compile-time capture, the curated suite in
``tools/bench_suites.py``). We REUSE it for building. Every benchmark *run*
(molt binary AND the CPython baseline) is timed through ``tools/safe_run.py
--json``, which enforces the RSS cap + wall-clock timeout the project mandates
for raw-binary execution and reports ``peak_rss_mib`` + ``elapsed_s`` for both
runtimes — satisfying the constitution's peak-RSS column for free.

Methodology (pyperf discipline)
-------------------------------
  * >= 5 measured samples per (benchmark, runtime) phase (configurable).
  * median + stdev + coefficient-of-variation outlier/instability detection.
  * COLD (first cold-cache run) AND WARM (steady-state) both captured — the
    constitution forbids warm-only wins.
  * MOLT_SESSION_ID=perfscore, CARGO_TARGET_DIR=target/sessions/perfscore.

Backends / profiles (baseline run)
----------------------------------
  * native + llvm. WASM is build/link-only today (a known socket-import
    instantiation gap blocks its run-path) -> recorded as ``run-blocked``.
    Luau lives in ``tools/benchmark_luau_vs_cpython.py`` -> follow-up.
  * release-fast is the daily-contract profile (``--build-profile release`` in
    the CLI maps to the ``release-fast`` cargo profile for the backend).
    release-output is the incremental next fill.

PyPy / Codon columns are present-but-nullable in the schema; neither is
installed on this host. See ``docs/perf/SCOREBOARD.md`` for the toolchain arc.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import statistics
import subprocess
import sys
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = REPO_ROOT / "tools"
SRC_ROOT = REPO_ROOT / "src"

if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

# Reuse the canonical build + suite machinery rather than rebuilding a timing
# loop. bench.py owns the daemon batch build, the memory guard, and the
# binary-size / compile-time capture; bench_suites owns the curated suite.
import bench  # noqa: E402
import bench_suites  # noqa: E402
import harness_memory_guard  # noqa: E402

SAFE_RUN = TOOLS_ROOT / "safe_run.py"
SCOREBOARD_DIR = REPO_ROOT / "bench" / "scoreboard"

SCHEMA_VERSION = 2

# The constitution's session isolation (must be set before any build command).
PERFSCORE_SESSION_ID = "perfscore"

# RED threshold: a molt speedup strictly below this vs CPython is a contract
# violation. 1.00x means "exactly CPython"; anything below is slower => RED.
RED_THRESHOLD = 1.00

# Coefficient-of-variation (stdev/median) above this flags a run as unstable.
# The constitution requires instability detection; an unstable cell cannot be
# trusted to be GREEN and is gated like a RED.
UNSTABLE_CV = 0.20

# safe_run RSS cap + wall-clock timeout per run. Generous enough for the heavy
# benchmarks (class_hierarchy, bytes_find @ 2s CPython) without letting a
# runaway reach OOM territory.
DEFAULT_RUN_RSS_MB = 4096
DEFAULT_RUN_TIMEOUT_S = 120.0
# Tight RSS poll so short benchmarks still capture a representative peak.
SAFE_RUN_POLL_S = 0.01

DEFAULT_SAMPLES = 5
DEFAULT_WARMUP = 2

# WASM cannot be run on this host today (socket-import instantiation gap); we
# build+link only and mark the run-path blocked. Luau has its own harness.
RUN_BLOCKED_BACKENDS = {"wasm"}


# ---------------------------------------------------------------------------
# Backend / profile descriptors
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class BackendSpec:
    """A (target, backend) build lane.

    ``molt_backend`` is the value forced into ``MOLT_BACKEND`` so the daemon
    selects the right codegen (native Cranelift vs the inkwell/LLVM feature).
    ``build_target`` is the CLI ``--target`` (native vs wasm).
    """

    target: str  # logical target name in the scoreboard ("native", "wasm")
    backend: str  # codegen backend ("native", "llvm", "wasm")
    molt_backend: str | None  # MOLT_BACKEND env value, or None to leave unset
    build_target: str  # molt CLI --target


NATIVE_CRANELIFT = BackendSpec("native", "native", None, "native")
NATIVE_LLVM = BackendSpec("native", "llvm", "llvm", "native")
WASM = BackendSpec("wasm", "wasm", None, "wasm")

BACKENDS_BY_NAME = {
    "native": NATIVE_CRANELIFT,
    "llvm": NATIVE_LLVM,
    "wasm": WASM,
}

# CLI --build-profile value -> the cargo profile it resolves to (for the
# scoreboard label). "release" is the daily contract profile and maps to the
# release-fast cargo profile for the backend (see cli.py:_backend_profile).
PROFILE_BUILD_FLAG = {
    "release-fast": "release",
    "release-output": "release",  # same CLI flag; distinguished by env below
    "dev-fast": "dev",
}


def _llvm_sys_prefix() -> str | None:
    """Resolve the LLVM_SYS prefix the inkwell backend build needs.

    The brew default is llvm@22 which is the WRONG version for this tree
    (llvm-sys 211 expects LLVM 21). Prefer an already-exported value, else
    fall back to the canonical llvm@21 cellar path.
    """
    explicit = os.environ.get("LLVM_SYS_211_PREFIX", "").strip()
    if explicit:
        return explicit
    candidate = Path("/opt/homebrew/opt/llvm@21")
    if candidate.exists():
        return str(candidate)
    return None


# ---------------------------------------------------------------------------
# Run timing via safe_run.py (RSS cap + timeout + peak-RSS, for EVERY binary)
# ---------------------------------------------------------------------------


@dataclass
class RunOutcome:
    ok: bool
    elapsed_s: float | None
    peak_rss_mib: float | None
    status: str  # "ok" | "timeout" | "oom" | "error" | "nonzero"
    exit_code: int | None
    stdout: str | None = None


def _safe_run_json(
    cmd: list[str],
    *,
    env: dict[str, str],
    rss_mb: int,
    timeout_s: float,
    label: str,
    capture_stdout: bool = False,
) -> RunOutcome:
    """Time one process through safe_run.py --json (RSS cap + timeout).

    safe_run forwards the child's stdout live and reports status as a single
    ``SAFE_RUN {json}`` line on stderr. We parse that line for elapsed_s +
    peak_rss_mib. When ``capture_stdout`` is set the child's stdout is captured
    here (for the one-time output-parity sanity); the repeated timed runs leave
    it streaming so safe_run's accounting stays honest.
    """
    full = [
        sys.executable,
        str(SAFE_RUN),
        "--json",
        "--rss-mb",
        str(rss_mb),
        "--timeout",
        str(timeout_s),
        "--poll",
        str(SAFE_RUN_POLL_S),
        "--label",
        label,
        "--",
        *cmd,
    ]
    try:
        proc = subprocess.run(
            full,
            env=env,
            stdout=subprocess.PIPE if capture_stdout else subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
            timeout=timeout_s + 30.0,
        )
    except subprocess.TimeoutExpired:
        return RunOutcome(False, None, None, "timeout", None)

    payload = _parse_safe_run_line(proc.stderr or "")
    if payload is None:
        return RunOutcome(False, None, None, "error", proc.returncode)
    status = payload.get("status", "error")
    elapsed = payload.get("elapsed_s")
    peak = payload.get("peak_rss_mib")
    exit_code = payload.get("exit")
    ok = status == "ok" and isinstance(exit_code, int) and exit_code == 0
    if not ok and status == "ok":
        status = "nonzero"
    return RunOutcome(
        ok=bool(ok),
        elapsed_s=float(elapsed) if isinstance(elapsed, (int, float)) else None,
        peak_rss_mib=float(peak) if isinstance(peak, (int, float)) else None,
        status=status,
        exit_code=exit_code if isinstance(exit_code, int) else None,
        stdout=proc.stdout if capture_stdout else None,
    )


def _parse_safe_run_line(stderr_text: str) -> dict | None:
    for line in reversed(stderr_text.splitlines()):
        line = line.strip()
        if line.startswith("SAFE_RUN ") and line[9:].lstrip().startswith("{"):
            try:
                return json.loads(line[9:].lstrip())
            except json.JSONDecodeError:
                continue
    return None


# ---------------------------------------------------------------------------
# Statistics — median, stdev, coefficient of variation, stability
# ---------------------------------------------------------------------------


@dataclass
class PhaseStats:
    samples_s: list[float] = field(default_factory=list)
    median_s: float | None = None
    mean_s: float | None = None
    stdev_s: float | None = None
    cv: float | None = None  # coefficient of variation (stdev / median)
    min_s: float | None = None
    max_s: float | None = None
    peak_rss_mib: float | None = None
    stable: bool = False
    n: int = 0

    @classmethod
    def from_runs(cls, runs: list[RunOutcome]) -> "PhaseStats":
        oks = [r for r in runs if r.ok and r.elapsed_s is not None]
        samples = [r.elapsed_s for r in oks if r.elapsed_s is not None]
        peaks = [r.peak_rss_mib for r in oks if r.peak_rss_mib is not None]
        if not samples:
            return cls(samples_s=[], n=0, stable=False)
        median = statistics.median(samples)
        mean = statistics.mean(samples)
        stdev = statistics.stdev(samples) if len(samples) > 1 else 0.0
        cv = (stdev / median) if median > 0 else None
        stable = (cv is not None and cv <= UNSTABLE_CV) and len(samples) >= 2
        return cls(
            samples_s=[round(s, 6) for s in samples],
            median_s=round(median, 6),
            mean_s=round(mean, 6),
            stdev_s=round(stdev, 6),
            cv=round(cv, 4) if cv is not None else None,
            min_s=round(min(samples), 6),
            max_s=round(max(samples), 6),
            peak_rss_mib=round(max(peaks), 1) if peaks else None,
            stable=stable,
            n=len(samples),
        )


# ---------------------------------------------------------------------------
# One scoreboard cell: benchmark x target x backend x profile
# ---------------------------------------------------------------------------


@dataclass
class Cell:
    benchmark: str
    target: str
    backend: str
    profile: str

    # Build facts (from the daemon batch build).
    build_ok: bool = False
    binary_size_kib: float | None = None
    compile_time_s: float | None = None

    # Run facts.
    run_blocked: bool = False
    run_blocked_reason: str | None = None
    molt_ok: bool = False
    cpython_ok: bool = False

    # COLD: first cold-cache sample for each runtime.
    cold_molt_s: float | None = None
    cold_cpython_s: float | None = None
    cold_ratio: float | None = None  # cpython / molt (>1 = molt faster)

    # WARM: steady-state median for each runtime.
    warm_molt_s: float | None = None
    warm_cpython_s: float | None = None
    warm_ratio: float | None = None

    # Headline speedup = warm cpython / warm molt (the constitution's column).
    cpython_ratio: float | None = None

    # Peak RSS (the worse of cold/warm samples) for each runtime.
    molt_peak_rss_mib: float | None = None
    cpython_peak_rss_mib: float | None = None

    # Stability + status.
    stable: bool = False
    red: bool = False
    status: str = (
        "pending"  # green | red | unstable | run-blocked | build-failed | error
    )
    note: str | None = None

    # Reserved for the follow-up toolchain arc (nullable today).
    pypy_ratio: float | None = None
    codon_ratio: float | None = None

    # When the CPython BASELINE itself cannot run the script (e.g. a
    # molt-internal benchmark that imports molt-only modules, or needs args the
    # bare CPython invocation lacks), there is no valid floor to compare
    # against. Such a cell is NOT red — it is excluded from the gate as
    # cpython-incompatible (a benchmark-harness fact, not a molt slowness fact).
    cpython_incompatible: bool = False

    # Provenance.
    output_parity: bool | None = None
    molt_stats: dict | None = None
    cpython_stats: dict | None = None
    log_artifact: str | None = None

    def finalize(self) -> None:
        """Derive ratios + RED status from the collected facts."""
        if self.run_blocked:
            self.status = "run-blocked"
            self.red = False
            return
        if not self.build_ok:
            self.status = "build-failed"
            self.red = True
            return
        # CPython baseline can't run -> no valid floor; not gated.
        if not self.cpython_ok:
            self.cpython_incompatible = True
            self.status = "cpython-incompatible"
            self.red = False
            if self.note is None:
                self.note = "CPython baseline could not run this script standalone"
            return
        # CPython runs but molt does not -> a real molt run failure (RED).
        if not self.molt_ok:
            self.status = "error"
            self.red = True
            if self.note is None:
                self.note = "molt run failed/unmeasurable while CPython ran"
            return

        self.cold_ratio = _safe_ratio(self.cold_cpython_s, self.cold_molt_s)
        self.warm_ratio = _safe_ratio(self.warm_cpython_s, self.warm_molt_s)
        self.cpython_ratio = self.warm_ratio

        # A cell is RED if either the warm OR cold speedup violates the floor —
        # the constitution forbids warm-only wins, so a cold-slow benchmark is
        # still a contract violation.
        warm_red = self.warm_ratio is not None and self.warm_ratio < RED_THRESHOLD
        cold_red = self.cold_ratio is not None and self.cold_ratio < RED_THRESHOLD

        if not self.stable:
            self.status = "unstable"
            self.red = True  # unstable-unmeasurable is gated like RED
            return
        if warm_red or cold_red:
            self.status = "red"
            self.red = True
            return
        self.status = "green"
        self.red = False


def _safe_ratio(numerator: float | None, denominator: float | None) -> float | None:
    if numerator is None or denominator is None or denominator <= 0:
        return None
    return round(numerator / denominator, 4)


def _cell_from_dict(d: dict) -> Cell:
    """Rehydrate a Cell from a stored per-cell dict (for --rebuild-summary).

    Filters to known dataclass fields so a stored board from an older schema
    still loads; missing fields fall back to the Cell defaults.
    """
    import dataclasses

    known = {f.name for f in dataclasses.fields(Cell)}
    return Cell(**{k: v for k, v in d.items() if k in known})


# ---------------------------------------------------------------------------
# Measurement driver
# ---------------------------------------------------------------------------


def _perfscore_build_env(spec: BackendSpec) -> dict[str, str]:
    """Build the conformance/build env for a backend lane.

    Sets the constitution's session isolation + the LLVM_SYS prefix + the
    MOLT_BACKEND selector. bench._canonical_bench_env folds in the molt
    conformance env (PYTHONPATH, codec, conformance dirs).
    """
    base = os.environ.copy()
    base["MOLT_SESSION_ID"] = PERFSCORE_SESSION_ID
    base["CARGO_TARGET_DIR"] = str(
        REPO_ROOT / "target" / "sessions" / PERFSCORE_SESSION_ID
    )
    if spec.molt_backend is not None:
        base["MOLT_BACKEND"] = spec.molt_backend
    else:
        base.pop("MOLT_BACKEND", None)
    if spec.backend == "llvm":
        prefix = _llvm_sys_prefix()
        if prefix:
            base["LLVM_SYS_211_PREFIX"] = prefix
    env = bench._canonical_bench_env(base)
    return env


def _cpython_run_env() -> dict[str, str]:
    """Env for the CPython baseline — src on PYTHONPATH, deterministic hashing."""
    env = bench._base_python_env()
    env["MOLT_SESSION_ID"] = PERFSCORE_SESSION_ID
    return env


def measure_cell(
    *,
    script_path: Path,
    spec: BackendSpec,
    profile: str,
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    batch_server: bench._BenchBatchBuildServer | None,
    cpython_bin: str,
    log_dir: Path,
) -> Cell:
    """Build + time one (benchmark, target, backend, profile) cell."""
    benchmark = bench_suites.canonical_benchmark_key(script_path)
    cell = Cell(
        benchmark=benchmark, target=spec.target, backend=spec.backend, profile=profile
    )
    log_path = log_dir / f"{Path(benchmark).stem}__{spec.backend}__{profile}.log"
    cell.log_artifact = str(log_path.relative_to(REPO_ROOT))
    log_lines: list[str] = [f"# {benchmark} | {spec.backend} | {profile}"]

    build_env = _perfscore_build_env(spec)
    extra_args = bench_suites.molt_args_for_benchmark(script_path)
    build_flag = PROFILE_BUILD_FLAG.get(profile, "release")

    # --- Build the molt binary via the canonical daemon batch build ---------
    binary = None
    if spec.build_target == "native":
        try:
            binary = bench.prepare_molt_binary(
                str(script_path),
                extra_args=extra_args,
                env=build_env,
                build_profile=build_flag,
                batch_server=batch_server,
                build_timeout_s=600.0,
            )
        except Exception as exc:  # noqa: BLE001 - record, never crash the sweep
            log_lines.append(f"BUILD EXCEPTION: {exc!r}")
            binary = None
    else:
        # WASM build/link only — produced via the CLI, not run here.
        binary = _build_wasm_only(script_path, build_env, build_flag, log_lines)

    if binary is None:
        cell.build_ok = False
        log_lines.append("BUILD FAILED")
        cell.finalize()
        _write_log(log_path, log_lines)
        return cell

    cell.build_ok = True
    cell.binary_size_kib = round(binary.size_kb, 1)
    cell.compile_time_s = round(binary.build_s, 3)
    log_lines.append(
        f"build_ok size_kib={cell.binary_size_kib} compile_s={cell.compile_time_s}"
    )

    if spec.backend in RUN_BLOCKED_BACKENDS:
        cell.run_blocked = True
        cell.run_blocked_reason = (
            "wasm run-path blocked: socket-import instantiation gap (build/link only)"
        )
        log_lines.append(f"RUN-BLOCKED: {cell.run_blocked_reason}")
        cell.finalize()
        _write_log(log_path, log_lines)
        _release_binary(binary)
        return cell

    run_args = bench.resolve_benchmark_run_args(str(script_path))
    molt_run_env = build_env

    # --- COLD samples (fresh cache; capture stdout once for parity) ---------
    cold_molt = _safe_run_json(
        [str(binary.path), *run_args],
        env=molt_run_env,
        rss_mb=rss_mb,
        timeout_s=timeout_s,
        label=f"molt-cold:{benchmark}",
        capture_stdout=True,
    )
    cold_cpy = _safe_run_json(
        [cpython_bin, str(script_path), *run_args],
        env=_cpython_run_env(),
        rss_mb=rss_mb,
        timeout_s=timeout_s,
        label=f"cpython-cold:{benchmark}",
        capture_stdout=True,
    )

    # --- WARM samples (warmup then >= `samples` measured) -------------------
    for _ in range(warmup):
        _safe_run_json(
            [str(binary.path), *run_args],
            env=molt_run_env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"molt-warmup:{benchmark}",
        )
    molt_runs = [
        _safe_run_json(
            [str(binary.path), *run_args],
            env=molt_run_env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"molt-warm:{benchmark}",
        )
        for _ in range(samples)
    ]

    for _ in range(warmup):
        _safe_run_json(
            [cpython_bin, str(script_path), *run_args],
            env=_cpython_run_env(),
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"cpython-warmup:{benchmark}",
        )
    cpy_runs = [
        _safe_run_json(
            [cpython_bin, str(script_path), *run_args],
            env=_cpython_run_env(),
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"cpython-warm:{benchmark}",
        )
        for _ in range(samples)
    ]

    _release_binary(binary)

    molt_stats = PhaseStats.from_runs(molt_runs)
    cpy_stats = PhaseStats.from_runs(cpy_runs)
    cell.molt_stats = asdict(molt_stats)
    cell.cpython_stats = asdict(cpy_stats)
    cell.molt_ok = molt_stats.n > 0 and cold_molt.ok
    cell.cpython_ok = cpy_stats.n > 0 and cold_cpy.ok

    cell.cold_molt_s = round(cold_molt.elapsed_s, 6) if cold_molt.elapsed_s else None
    cell.cold_cpython_s = round(cold_cpy.elapsed_s, 6) if cold_cpy.elapsed_s else None
    cell.warm_molt_s = molt_stats.median_s
    cell.warm_cpython_s = cpy_stats.median_s

    cell.molt_peak_rss_mib = _max_opt(molt_stats.peak_rss_mib, cold_molt.peak_rss_mib)
    cell.cpython_peak_rss_mib = _max_opt(cpy_stats.peak_rss_mib, cold_cpy.peak_rss_mib)

    # Both phases must be stable for the cell to be trusted GREEN.
    cell.stable = molt_stats.stable and cpy_stats.stable

    # One-time output parity (informational; not the gate).
    if cold_molt.stdout is not None and cold_cpy.stdout is not None:
        cell.output_parity = cold_molt.stdout.strip() == cold_cpy.stdout.strip()

    if not cell.molt_ok:
        cell.note = f"molt run unmeasurable (status={cold_molt.status})"
    elif not cell.cpython_ok:
        cell.note = f"cpython run unmeasurable (status={cold_cpy.status})"

    cell.finalize()
    log_lines.append(
        f"molt warm_median={cell.warm_molt_s} cpython warm_median={cell.warm_cpython_s} "
        f"speedup={cell.cpython_ratio} status={cell.status}"
    )
    _write_log(log_path, log_lines)
    return cell


def _build_wasm_only(
    script_path: Path,
    build_env: dict[str, str],
    build_flag: str,
    log_lines: list[str],
) -> bench.MoltBinary | None:
    """Build+link a WASM artifact (run-path is blocked; we only verify it links).

    Uses the molt CLI directly with ``--target wasm``. Returns a MoltBinary-like
    handle whose ``path`` is the .wasm and whose size/compile-time are captured,
    so the scoreboard records build-facts for the WASM lane.
    """
    import tempfile

    out_dir = Path(
        tempfile.mkdtemp(prefix="perfscore-wasm-", dir=str(bench.BENCH_TMP_ROOT))
    )
    cmd = [
        *bench._molt_build_cmd(build_flag),
        "--target",
        "wasm",
        "--trusted",
        "--json",
        "--rebuild",
        "--out-dir",
        str(out_dir),
        str(script_path),
    ]
    start = time.perf_counter()
    try:
        res = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_BENCH",
            env=build_env,
            capture_output=True,
            text=True,
            timeout=600.0,
        )
    except Exception as exc:  # noqa: BLE001
        log_lines.append(f"WASM BUILD EXCEPTION: {exc!r}")
        return None
    build_s = time.perf_counter() - start
    if res.returncode != 0:
        tail = (res.stderr or res.stdout or "").strip()[-2000:]
        log_lines.append(f"WASM BUILD FAILED rc={res.returncode}\n{tail}")
        return None
    try:
        payload = json.loads((res.stdout or "{}").strip() or "{}")
    except json.JSONDecodeError:
        log_lines.append("WASM BUILD: non-JSON stdout")
        return None
    out_str = payload.get("data", {}).get("output") or payload.get("output")
    if not out_str:
        # Fall back to scanning the out dir for a .wasm artifact.
        wasms = list(out_dir.rglob("*.wasm"))
        if not wasms:
            log_lines.append("WASM BUILD: no .wasm artifact")
            return None
        out_path = wasms[0]
    else:
        out_path = Path(out_str)
        if not out_path.exists():
            wasms = list(out_dir.rglob("*.wasm"))
            out_path = wasms[0] if wasms else out_path
    if not out_path.exists():
        log_lines.append(f"WASM BUILD: artifact missing {out_path}")
        return None
    size_kb = out_path.stat().st_size / 1024

    class _TmpHolder:
        def cleanup(self) -> None:
            import shutil

            shutil.rmtree(out_dir, ignore_errors=True)

    return bench.MoltBinary(out_path, _TmpHolder(), build_s, size_kb)


def _release_binary(binary: bench.MoltBinary) -> None:
    holder = getattr(binary, "temp_dir", None)
    if holder is not None:
        try:
            holder.cleanup()
        except Exception:  # noqa: BLE001
            pass


def _max_opt(a: float | None, b: float | None) -> float | None:
    vals = [v for v in (a, b) if v is not None]
    return max(vals) if vals else None


def _write_log(path: Path, lines: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


# ---------------------------------------------------------------------------
# Scoreboard assembly + JSON schema
# ---------------------------------------------------------------------------


def _git_rev() -> str:
    return bench._git_rev() or "unknown"


def build_scoreboard_doc(
    cells: list[Cell],
    *,
    benchmarks_run: list[str],
    benchmarks_deferred: list[dict],
    cpython_version: str,
    samples: int,
    warmup: int,
) -> dict:
    """Assemble the nested machine-readable scoreboard.

    Shape: ``benchmark -> target -> backend -> profile -> {cell fields}``.
    """
    git_rev = _git_rev()
    nested: dict = {}
    for cell in cells:
        d = asdict(cell)
        (
            nested.setdefault(cell.benchmark, {})
            .setdefault(cell.target, {})
            .setdefault(cell.backend, {})[cell.profile]
        ) = d

    red_cells = [c for c in cells if c.red]
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "cpython_floor_scoreboard",
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": git_rev,
        "host": {
            "platform": sys.platform,
            "python_runner": sys.version.split()[0],
            "cpython_baseline": cpython_version,
        },
        "direction": "speedup = cpython_time / molt_time; >1.0 = molt faster; <1.0 = RED",
        "red_threshold": RED_THRESHOLD,
        "unstable_cv_threshold": UNSTABLE_CV,
        "methodology": {
            "samples_per_phase": samples,
            "warmup_runs": warmup,
            "cold_and_warm": True,
            "run_guard": "tools/safe_run.py --json (rss cap + timeout + peak rss)",
            "build": "tools/bench.py daemon batch build (memory-guarded)",
        },
        "reserved_columns": {
            "pypy_ratio": "nullable — PyPy not installed; follow-up toolchain arc",
            "codon_ratio": "nullable — Codon not installed; follow-up toolchain arc",
        },
        "summary": {
            "cells_total": len(cells),
            "cells_green": sum(1 for c in cells if c.status == "green"),
            "cells_red": sum(1 for c in cells if c.status == "red"),
            "cells_unstable": sum(1 for c in cells if c.status == "unstable"),
            "cells_build_failed": sum(1 for c in cells if c.status == "build-failed"),
            "cells_run_blocked": sum(1 for c in cells if c.status == "run-blocked"),
            "cells_error": sum(1 for c in cells if c.status == "error"),
            "cells_cpython_incompatible": sum(
                1 for c in cells if c.status == "cpython-incompatible"
            ),
            "any_red": bool(red_cells),
            # Triage split: warm-red = genuinely slow steady-state (a real
            # representation gap, constitution triage #1); cold-only-red = warm
            # is >= floor but cold-start tax sinks the cold path (the separate
            # binary-size / cold-start column). Both are RED per "no warm-only
            # wins", but they route to different fix lanes.
            "red_breakdown": {
                "warm_red": sorted(
                    _cell_key(asdict(c))
                    for c in red_cells
                    if c.warm_ratio is not None and c.warm_ratio < RED_THRESHOLD
                ),
                "cold_only_red": sorted(
                    _cell_key(asdict(c))
                    for c in red_cells
                    if c.status == "red"
                    and (c.warm_ratio is not None and c.warm_ratio >= RED_THRESHOLD)
                    and (c.cold_ratio is not None and c.cold_ratio < RED_THRESHOLD)
                ),
                "unstable": sorted(
                    _cell_key(asdict(c)) for c in cells if c.status == "unstable"
                ),
                "build_failed_or_error": sorted(
                    _cell_key(asdict(c))
                    for c in cells
                    if c.status in ("build-failed", "error")
                ),
                "cpython_incompatible": sorted(
                    _cell_key(asdict(c))
                    for c in cells
                    if c.status == "cpython-incompatible"
                ),
            },
        },
        "benchmarks_run": benchmarks_run,
        "benchmarks_deferred": benchmarks_deferred,
        "scoreboard": nested,
    }


def _validate_schema(doc: dict) -> list[str]:
    """Assert the emitted board matches the constitution schema. Returns problems."""
    problems: list[str] = []
    required_top = {
        "schema_version",
        "kind",
        "generated_at",
        "git_rev",
        "host",
        "direction",
        "red_threshold",
        "methodology",
        "reserved_columns",
        "summary",
        "benchmarks_run",
        "benchmarks_deferred",
        "scoreboard",
    }
    missing = required_top - set(doc)
    if missing:
        problems.append(f"missing top-level keys: {sorted(missing)}")
    # JSON round-trips.
    try:
        json.loads(json.dumps(doc))
    except (TypeError, ValueError) as exc:
        problems.append(f"doc is not JSON-serializable: {exc}")
    # Reserved nullable columns must be present in every cell.
    required_cell = {
        "benchmark",
        "target",
        "backend",
        "profile",
        "cpython_ratio",
        "cold_ratio",
        "warm_ratio",
        "binary_size_kib",
        "molt_peak_rss_mib",
        "compile_time_s",
        "stable",
        "red",
        "status",
        "pypy_ratio",
        "codon_ratio",
        "log_artifact",
    }
    cells = _flatten_cells(doc)
    if not cells:
        problems.append("no cells emitted")
    for c in cells:
        cmiss = required_cell - set(c)
        if cmiss:
            problems.append(
                f"cell {c.get('benchmark')} missing fields: {sorted(cmiss)}"
            )
            break
        if c.get("pypy_ratio") is not None or c.get("codon_ratio") is not None:
            problems.append(
                "pypy_ratio/codon_ratio should be null until the follow-up arc"
            )
            break
    return problems


# ---------------------------------------------------------------------------
# Human-readable summary (RED rows first)
# ---------------------------------------------------------------------------


def print_summary(doc: dict) -> None:
    cells = _flatten_cells(doc)

    def sort_key(c: dict) -> tuple:
        order = {
            "red": 0,
            "build-failed": 0,
            "error": 0,
            "unstable": 1,
            "run-blocked": 2,
            "cpython-incompatible": 2,
            "green": 3,
        }
        return (
            order.get(c["status"], 4),
            -(c.get("cpython_ratio") or 0.0),
            c["benchmark"],
        )

    cells.sort(key=sort_key)

    print("\n" + "=" * 100)
    print(
        "CPYTHON FLOOR SCOREBOARD  (speedup = cpython/molt; <1.00 = RED contract violation)"
    )
    print(f"git_rev={doc['git_rev']}  cpython={doc['host']['cpython_baseline']}")
    print("=" * 100)
    hdr = f"{'STATUS':<13}{'SPEEDUP':>9}  {'COLD':>7}  {'WARM':>7}  {'SIZEKiB':>8}  {'RSSMiB':>7}  {'CMP_s':>6}  BENCHMARK [backend/profile]"
    print(hdr)
    print("-" * 100)
    for c in cells:
        speed = c.get("cpython_ratio")
        cold = c.get("cold_ratio")
        warm = c.get("warm_ratio")
        size = c.get("binary_size_kib")
        rss = c.get("molt_peak_rss_mib")
        cmp_s = c.get("compile_time_s")
        flag = {
            "red": "RED",
            "green": "ok",
            "unstable": "UNSTABLE",
            "build-failed": "BUILD-FAIL",
            "run-blocked": "run-blocked",
            "cpython-incompatible": "cpy-incompat",
            "error": "ERROR",
        }.get(c["status"], c["status"])
        print(
            f"{flag:<13}"
            f"{_fmt(speed):>9}  "
            f"{_fmt(cold):>7}  "
            f"{_fmt(warm):>7}  "
            f"{_fmt(size, 0):>8}  "
            f"{_fmt(rss, 0):>7}  "
            f"{_fmt(cmp_s, 1):>6}  "
            f"{c['benchmark']} [{c['backend']}/{c['profile']}]"
        )
    print("-" * 100)
    s = doc["summary"]
    print(
        f"TOTAL={s['cells_total']}  GREEN={s['cells_green']}  RED={s['cells_red']}  "
        f"UNSTABLE={s['cells_unstable']}  BUILD-FAIL={s['cells_build_failed']}  "
        f"RUN-BLOCKED={s['cells_run_blocked']}  ERROR={s['cells_error']}  "
        f"CPY-INCOMPAT={s.get('cells_cpython_incompatible', 0)}"
    )
    rb = s.get("red_breakdown", {})
    warm_red = rb.get("warm_red", [])
    cold_only = rb.get("cold_only_red", [])
    print(
        f"  RED split: warm-red (genuine steady-state gap)={len(warm_red)}  "
        f"cold-only-red (startup tax)={len(cold_only)}"
    )
    if warm_red:
        print("  WARM-RED (constitution triage #1 — slow even at steady state):")
        for k in warm_red:
            print(f"    {k}")
    if doc["benchmarks_deferred"]:
        print(f"DEFERRED ({len(doc['benchmarks_deferred'])}):")
        for d in doc["benchmarks_deferred"]:
            print(f"  - {d['benchmark']}: {d['reason']}")
    print("=" * 100 + "\n")


def _flatten_cells(doc: dict) -> list[dict]:
    out: list[dict] = []
    for _bm, targets in doc["scoreboard"].items():
        for _t, backends in targets.items():
            for _b, profiles in backends.items():
                for _p, cell in profiles.items():
                    out.append(cell)
    return out


def _fmt(v: float | None, places: int = 2) -> str:
    if v is None:
        return "-"
    if places == 0:
        return f"{v:.0f}"
    return f"{v:.{places}f}"


# ---------------------------------------------------------------------------
# Baseline diff mode
# ---------------------------------------------------------------------------


def diff_against_baseline(
    doc: dict, baseline_path: Path
) -> tuple[list[str], list[str]]:
    """Return (newly_red, regressed_still_green) message lists vs a prior board."""
    try:
        prior = json.loads(baseline_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return ([f"baseline unreadable: {exc}"], [])

    prior_cells = {_cell_key(c): c for c in _flatten_cells(prior)}
    newly_red: list[str] = []
    regressed: list[str] = []
    for c in _flatten_cells(doc):
        key = _cell_key(c)
        old = prior_cells.get(key)
        if old is None:
            continue
        new_ratio = c.get("cpython_ratio")
        old_ratio = old.get("cpython_ratio")
        if c.get("red") and not old.get("red"):
            newly_red.append(
                f"{key}: NEWLY RED  {_fmt(old_ratio)} -> {_fmt(new_ratio)}"
            )
        elif (
            new_ratio is not None
            and old_ratio is not None
            and not c.get("red")
            and new_ratio < old_ratio * 0.95  # >5% slower but still green
        ):
            regressed.append(
                f"{key}: regressed-but-green  {_fmt(old_ratio)} -> {_fmt(new_ratio)} "
                f"({(new_ratio / old_ratio - 1) * 100:+.1f}%)"
            )
    return newly_red, regressed


def _cell_key(c: dict) -> str:
    return f"{c['benchmark']} [{c['backend']}/{c['profile']}]"


def _latest_baseline() -> Path | None:
    if not SCOREBOARD_DIR.exists():
        return None
    candidates = sorted(SCOREBOARD_DIR.glob("cpython_*.json"))
    return candidates[-1] if candidates else None


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _resolve_benchmark_set(name: str, explicit: list[str] | None) -> list[Path]:
    if explicit:
        return [_resolve_script(b) for b in explicit]
    if name == "smoke":
        keys = bench_suites.SMOKE_BENCHMARKS
    elif name == "core":
        keys = bench_suites.BENCHMARKS
    else:
        raise SystemExit(f"unknown benchmark set: {name}")
    return [_resolve_script(k) for k in keys]


def _resolve_script(key: str) -> Path:
    p = Path(key)
    if p.is_absolute() and p.exists():
        return p
    cand = REPO_ROOT / key
    if cand.exists():
        return cand
    cand2 = REPO_ROOT / "tests" / "benchmarks" / Path(key).name
    if cand2.exists():
        return cand2
    raise SystemExit(f"benchmark not found: {key}")


def _rebuild_summary(path: Path, *, no_gate: bool) -> int:
    """Re-derive a stored board's summary/breakdown/gate from its per-cell data.

    Loads the authoritative per-cell measurements, re-runs them through the
    CURRENT ``build_scoreboard_doc`` (so the summary + red_breakdown match the
    current tool), writes the board back in place, prints the summary, and
    returns the gate exit code. No binaries are rebuilt; no benchmarks re-run.
    """
    try:
        prior = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"--rebuild-summary: cannot read {path}: {exc}", file=sys.stderr)
        return 2
    cells = [_cell_from_dict(c) for c in _flatten_cells(prior)]
    # Re-run the classifier on the stored measurements so status/red reflect the
    # CURRENT finalize() logic (e.g. the cpython-incompatible reclassification),
    # not whatever the board was stamped with at measurement time.
    for cell in cells:
        cell.finalize()
    method = prior.get("methodology", {})
    # Re-derive the deferred list from cpython-incompatible cells.
    deferred = list(prior.get("benchmarks_deferred", []))
    for cell in cells:
        if cell.status == "cpython-incompatible":
            dkey = f"{cell.benchmark} [{cell.backend}/{cell.profile}]"
            if not any(d.get("benchmark") == dkey for d in deferred):
                deferred.append(
                    {
                        "benchmark": dkey,
                        "reason": cell.note
                        or "CPython baseline could not run this script",
                    }
                )
    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=prior.get("benchmarks_run", []),
        benchmarks_deferred=deferred,
        cpython_version=prior.get("host", {}).get("cpython_baseline", "unknown"),
        samples=method.get("samples_per_phase", DEFAULT_SAMPLES),
        warmup=method.get("warmup_runs", DEFAULT_WARMUP),
    )
    # Preserve the original generation timestamp + git_rev of the measurement.
    doc["generated_at"] = prior.get("generated_at", doc["generated_at"])
    doc["git_rev"] = prior.get("git_rev", doc["git_rev"])
    if "host" in prior:
        doc["host"] = prior["host"]
    path.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    print(f"[rebuild-summary] rewrote {path}", file=sys.stderr)
    print_summary(doc)
    if no_gate:
        return 0
    return 1 if doc["summary"]["any_red"] else 0


def _merge_boards(sources: list[Path], out: Path, *, no_gate: bool) -> int:
    """Merge per-cell data from multiple scoreboard JSONs into one board.

    Used to combine separately-run backend lanes (e.g. native + llvm) into the
    single ``cpython_<gitrev>.json`` the constitution mandates, without
    re-measuring either lane. Cells are keyed (benchmark, target, backend,
    profile); a later source overrides an earlier one for the same key.
    """
    by_key: dict[tuple, Cell] = {}
    benchmarks_run: list[str] = []
    deferred: list[dict] = []
    host: dict = {}
    method: dict = {}
    cpython_version = "unknown"
    git_rev = "unknown"
    generated_at = None
    for src in sources:
        try:
            doc = json.loads(src.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            print(f"--merge: cannot read {src}: {exc}", file=sys.stderr)
            return 2
        host = doc.get("host", host)
        method = doc.get("methodology", method)
        cpython_version = host.get("cpython_baseline", cpython_version)
        git_rev = doc.get("git_rev", git_rev)
        generated_at = doc.get("generated_at", generated_at)
        for d in _flatten_cells(doc):
            cell = _cell_from_dict(d)
            cell.finalize()
            by_key[(cell.benchmark, cell.target, cell.backend, cell.profile)] = cell
        for b in doc.get("benchmarks_run", []):
            if b not in benchmarks_run:
                benchmarks_run.append(b)
    cells = list(by_key.values())
    for cell in cells:
        if cell.status == "cpython-incompatible":
            dkey = f"{cell.benchmark} [{cell.backend}/{cell.profile}]"
            if not any(x.get("benchmark") == dkey for x in deferred):
                deferred.append(
                    {
                        "benchmark": dkey,
                        "reason": cell.note or "CPython baseline could not run",
                    }
                )
    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=sorted(benchmarks_run),
        benchmarks_deferred=deferred,
        cpython_version=cpython_version,
        samples=method.get("samples_per_phase", DEFAULT_SAMPLES),
        warmup=method.get("warmup_runs", DEFAULT_WARMUP),
    )
    doc["git_rev"] = git_rev
    if host:
        doc["host"] = host
    if generated_at:
        doc["generated_at"] = generated_at
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    print(
        f"[merge] {len(sources)} boards -> {out} ({len(cells)} cells)", file=sys.stderr
    )
    print_summary(doc)
    if no_gate:
        return 0
    return 1 if doc["summary"]["any_red"] else 0


def main(argv: list[str]) -> int:
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
    parser.add_argument("--samples", type=int, default=DEFAULT_SAMPLES)
    parser.add_argument("--warmup", type=int, default=DEFAULT_WARMUP)
    parser.add_argument("--rss-mb", type=int, default=DEFAULT_RUN_RSS_MB)
    parser.add_argument("--timeout", type=float, default=DEFAULT_RUN_TIMEOUT_S)
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
        return _rebuild_summary(Path(ns.rebuild_summary), no_gate=ns.no_gate)

    if ns.merge is not None:
        merge_out = (
            Path(ns.out) if ns.out else SCOREBOARD_DIR / f"cpython_{_git_rev()}.json"
        )
        return _merge_boards([Path(p) for p in ns.merge], merge_out, no_gate=ns.no_gate)

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

    scripts = _resolve_benchmark_set(ns.set, ns.benchmark)
    cpython_bin = _resolve_system_cpython(ns.cpython)
    cpython_version = _probe_cpython_version(cpython_bin)
    print(
        f"[scoreboard] CPython oracle: {cpython_bin} ({cpython_version})",
        file=sys.stderr,
    )

    git_rev = _git_rev()
    SCOREBOARD_DIR.mkdir(parents=True, exist_ok=True)
    out_path = Path(ns.out) if ns.out else SCOREBOARD_DIR / f"cpython_{git_rev}.json"
    log_dir = SCOREBOARD_DIR / f"logs_{git_rev}"
    partial_path = out_path.with_suffix(".partial.json")

    benchmarks_run: list[str] = []
    benchmarks_deferred: list[dict] = []
    cells: list[Cell] = []

    # Per (backend, profile) we open ONE daemon batch build server and reuse it
    # across the whole benchmark set — matching bench.py's amortized-build model.
    for backend_name in backends:
        spec = BACKENDS_BY_NAME[backend_name]
        for profile in profiles:
            batch_server = None
            if spec.build_target == "native":
                try:
                    batch_server = bench._BenchBatchBuildServer(
                        _perfscore_build_env(spec)
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
                    key = bench_suites.canonical_benchmark_key(script)
                    print(
                        f"[scoreboard] {key} | {backend_name} | {profile} ...",
                        file=sys.stderr,
                        flush=True,
                    )
                    cell = measure_cell(
                        script_path=script,
                        spec=spec,
                        profile=profile,
                        samples=ns.samples,
                        warmup=ns.warmup,
                        rss_mb=ns.rss_mb,
                        timeout_s=ns.timeout,
                        batch_server=batch_server,
                        cpython_bin=cpython_bin,
                        log_dir=log_dir,
                    )
                    cells.append(cell)
                    if key not in benchmarks_run:
                        benchmarks_run.append(key)
                    # CPython-incompatible benchmarks have no valid floor and
                    # are excluded from the gate — record the exclusion
                    # explicitly (no silent truncation).
                    if cell.status == "cpython-incompatible":
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
                    _checkpoint(
                        partial_path,
                        cells,
                        benchmarks_run,
                        benchmarks_deferred,
                        cpython_version,
                        ns.samples,
                        ns.warmup,
                    )
                    print(
                        f"    -> {cell.status}  speedup={_fmt(cell.cpython_ratio)}",
                        file=sys.stderr,
                        flush=True,
                    )
            finally:
                if batch_server is not None:
                    try:
                        batch_server.close()
                    except Exception:  # noqa: BLE001
                        pass

    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=benchmarks_run,
        benchmarks_deferred=benchmarks_deferred,
        cpython_version=cpython_version,
        samples=ns.samples,
        warmup=ns.warmup,
    )
    out_path.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    if partial_path.exists():
        partial_path.unlink()
    print(f"\nscoreboard JSON -> {out_path}", file=sys.stderr)

    if ns.self_test:
        problems = _validate_schema(doc)
        if problems:
            print("[self-test] SCHEMA VALIDATION FAILED:", file=sys.stderr)
            for p in problems:
                print(f"    - {p}", file=sys.stderr)
            return 3
        print(
            "[self-test] schema OK: required top-level keys + per-cell fields present, "
            "gate wired, JSON round-trips.",
            file=sys.stderr,
        )

    print_summary(doc)

    if ns.baseline is not None:
        baseline_path = (
            _latest_baseline() if ns.baseline == "__latest__" else Path(ns.baseline)
        )
        if baseline_path is None or not baseline_path.exists():
            print("[baseline] no prior scoreboard to diff against.", file=sys.stderr)
        else:
            newly_red, regressed = diff_against_baseline(doc, baseline_path)
            print(f"\n[baseline diff vs {baseline_path.name}]")
            if newly_red:
                print("  NEWLY RED:")
                for m in newly_red:
                    print(f"    {m}")
            if regressed:
                print("  REGRESSED (still green):")
                for m in regressed:
                    print(f"    {m}")
            if not newly_red and not regressed:
                print("  no new reds, no regressions.")

    any_red = doc["summary"]["any_red"]
    if ns.no_gate:
        return 0
    return 1 if any_red else 0


def _checkpoint(
    path: Path,
    cells: list[Cell],
    benchmarks_run: list[str],
    benchmarks_deferred: list[dict],
    cpython_version: str,
    samples: int,
    warmup: int,
) -> None:
    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=benchmarks_run,
        benchmarks_deferred=benchmarks_deferred,
        cpython_version=cpython_version,
        samples=samples,
        warmup=warmup,
    )
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)


def _resolve_system_cpython(explicit: str | None) -> str:
    """Resolve the CPython oracle/baseline to the SYSTEM python3 (3.14).

    The constitution pins the CPython floor to the system interpreter, NOT the
    runner's venv. When this tool is launched under ``uv run --python 3.12`` the
    venv python (3.12) would otherwise leak in via ``sys.executable`` and quietly
    move the floor. We therefore resolve the real system python3 explicitly and
    canonicalize it through the harness guard so the spawn boundary sees an
    absolute path.
    """
    import shutil

    if explicit:
        resolved = explicit
    else:
        # Prefer the homebrew system python3 (3.14 on this host); fall back to
        # whatever PATH calls python3 — but never the molt venv interpreter.
        candidates = [
            "/opt/homebrew/bin/python3",
            "/usr/local/bin/python3",
            shutil.which("python3") or "",
            "/usr/bin/python3",
        ]
        resolved = next(
            (
                c
                for c in candidates
                if c and Path(c).exists() and ".venv" not in c and "/sessions/" not in c
            ),
            "python3",
        )
    return bench._canonical_interpreter(resolved)


def _probe_cpython_version(cpython_bin: str) -> str:
    try:
        res = subprocess.run(
            [cpython_bin, "--version"],
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"
    out = (res.stdout or res.stderr or "").strip()
    return out.replace("Python ", "") or "unknown"


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
