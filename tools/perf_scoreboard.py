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
COLD_START_BUDGET_PATH = SCOREBOARD_DIR / "cold_start_budget.json"

# Schema 3 adds the two-dimensional verdict (warm vs cold), full provenance
# metadata (origin/local/merge-base SHAs, tool identity, backend binary
# identity, stdlib cache key, authoritative flag), and the cold-start budget
# fields. Boards written by schema-2 tools still load (missing fields default).
SCHEMA_VERSION = 3

# The constitution's session isolation (must be set before any build command).
PERFSCORE_SESSION_ID = "perfscore"

# RED threshold: a molt speedup strictly below this vs CPython is a contract
# violation. 1.00x means "exactly CPython"; anything below is slower => RED.
RED_THRESHOLD = 1.00

# Coefficient-of-variation (stdev/median) above this flags a run as unstable.
# The constitution requires instability detection; an unstable cell cannot be
# trusted to be GREEN and is gated like a RED.
UNSTABLE_CV = 0.20

# --- Two-dimensional verdict vocabulary (council ruling A) ------------------
# warm ≠ cold. A warm-slow cell is an EXECUTION-ENGINE red (needs an IR fact);
# a cold-slow-but-warm-fast cell is a fixed STARTUP TAX (needs runtime/artifact
# work) and must NOT be conflated with engine slowness. The single legacy
# ``red`` bool blended both — these verdicts split them.
VERDICT_GREEN = "GREEN"
VERDICT_FAIL_ENGINE = "FAIL_ENGINE"  # warm_speedup <= 1.00  (release blocker)
VERDICT_FAIL_COLD_BUDGET = "FAIL_COLD_BUDGET"  # startup_tax_ms > budget_ms
VERDICT_WARN_COLD_FLOOR = "WARN_COLD_FLOOR"  # cold<=1 & warm>1, tax is sole cause
VERDICT_FAIL_STALE = "FAIL_STALE"  # non-authoritative tree — overrides all
# Infrastructure verdicts (not engine/cold slowness — routed to their own lane).
VERDICT_BUILD_FAILED = "BUILD_FAILED"
VERDICT_RUN_ERROR = "RUN_ERROR"  # cpython ran, molt did not
VERDICT_UNSTABLE = "UNSTABLE"  # CV too high to trust either direction
VERDICT_RUN_BLOCKED = "RUN_BLOCKED"  # wasm run-path gap (build/link only)
VERDICT_CPY_INCOMPAT = "CPY_INCOMPATIBLE"  # no CPython floor to compare against

# Verdicts that FAIL the gate (nonzero exit). WARN_COLD_FLOOR does NOT fail
# unless --strict-cold; FAIL_STALE fails unless --allow-nonauthoritative.
GATE_FAILING_VERDICTS = frozenset(
    {
        VERDICT_FAIL_ENGINE,
        VERDICT_FAIL_COLD_BUDGET,
        VERDICT_BUILD_FAILED,
        VERDICT_RUN_ERROR,
        VERDICT_UNSTABLE,
    }
)


def _load_cold_start_budgets() -> dict:
    """Load the per (backend, profile) cold-start tax budgets in milliseconds.

    Shape: ``{"budgets": {"native/release-fast": {"budget_ms": N, ...}}}``.
    A missing file or missing cell entry means "no budget recorded yet" — the
    FAIL_COLD_BUDGET verdict cannot fire (we never invent a budget), and the
    board records the measured tax so the budget can be seeded from this run.
    """
    if not COLD_START_BUDGET_PATH.exists():
        return {"budgets": {}}
    try:
        return json.loads(COLD_START_BUDGET_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {"budgets": {}}


def _budget_ms_for(budgets: dict, backend: str, profile: str) -> float | None:
    entry = budgets.get("budgets", {}).get(f"{backend}/{profile}")
    if not isinstance(entry, dict):
        return None
    val = entry.get("budget_ms")
    return float(val) if isinstance(val, (int, float)) else None


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


def _robust_cell_stable(molt: PhaseStats, cpy: PhaseStats) -> bool:
    """Is the cell's warm verdict trustworthy despite CPython-side outliers?

    molt is the artifact under test and MUST be stable. CPython is the reference
    floor; a single CPython GC/scheduler spike must not throw out a cell where
    molt wins decisively and is itself stable. The cell is stable iff:
      * molt is stable (low CV), AND
      * EITHER CPython is also stable,
        OR the warm verdict is robust to CPython's FULL sample spread — i.e.
        the warm_speedup (cpython/molt, on the molt median) keeps the same
        side of the 1.00 floor whether computed with CPython's fastest
        (cpy.min_s) or slowest (cpy.max_s) sample. If both bounds agree on
        GREEN-or-RED, a CPython outlier cannot flip the verdict.
    """
    if not molt.stable:
        return False
    if cpy.stable:
        return True
    if (
        molt.median_s is None
        or cpy.min_s is None
        or cpy.max_s is None
        or molt.median_s <= 0
    ):
        return False
    # warm_speedup = cpython / molt. Using CPython's min and max bounds, does
    # the verdict (>1 vs <=1) stay consistent?
    lo = cpy.min_s / molt.median_s
    hi = cpy.max_s / molt.median_s
    both_win = lo > RED_THRESHOLD and hi > RED_THRESHOLD
    both_lose = lo <= RED_THRESHOLD and hi <= RED_THRESHOLD
    return both_win or both_lose


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

    # --- Two-dimensional verdict (council ruling A) ----------------------
    # warm_speedup = cpython_warm / molt_warm  (the EXECUTION-ENGINE axis).
    # cold_speedup = cpython_cold / molt_cold  (the END-TO-END cold axis).
    # startup_tax_ms = (molt_cold_total - molt_warm_total) * 1000  (fixed tax).
    warm_speedup: float | None = None
    cold_speedup: float | None = None
    startup_tax_ms: float | None = None
    cold_budget_ms: float | None = None  # the budget this cell was gated against
    verdict: str = "pending"  # one of the VERDICT_* constants
    # The single most-likely missing fact / startup component, for triage.
    suspected_missing_fact: str | None = None
    suspected_startup_component: str | None = None

    # --- PyPy / Codon comparator lanes (council Lane C; nullable) ---------
    # pypy_ratio = pypy_warm / molt_warm  (>1 = molt faster than PyPy).
    # codon_ratio = codon_warm / molt_warm (>1 = molt faster than Codon).
    pypy_ratio: float | None = None
    pypy_warm_s: float | None = None
    codon_ratio: float | None = None
    codon_warm_s: float | None = None
    # Codon is AOT but NOT drop-in for every benchmark; a non-equivalent
    # benchmark is NEVER scored win/loss — only recorded as "non-equivalent".
    codon_equivalent: bool | None = None
    codon_note: str | None = None

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

    def finalize(
        self,
        *,
        budget_ms: float | None = None,
        authoritative: bool = True,
    ) -> None:
        """Derive ratios + the TWO-DIMENSIONAL verdict from the collected facts.

        ``budget_ms`` is the cold-start tax budget for this (backend, profile)
        cell (None = no budget recorded yet; FAIL_COLD_BUDGET cannot fire).
        ``authoritative`` False stamps every cell FAIL_STALE (the tree is not
        origin/main) — overriding all other verdicts per council ruling A.

        Sets ``verdict`` (the VERDICT_* vocabulary) AND keeps ``status`` + the
        legacy ``red`` bool in sync so older consumers (diff, JSON readers) keep
        working. ``status`` mirrors the verdict in lowercase-legacy form.
        """
        self.cold_budget_ms = budget_ms

        # FAIL_STALE overrides everything: a non-authoritative tree's numbers
        # are not the origin/main contract, full stop.
        if not authoritative:
            self.verdict = VERDICT_FAIL_STALE
            self.status = "stale"
            self.red = True
            if self.note is None:
                self.note = "non-authoritative tree (local != origin/main or dirty)"
            return

        if self.run_blocked:
            self.verdict = VERDICT_RUN_BLOCKED
            self.status = "run-blocked"
            self.red = False
            return
        if not self.build_ok:
            self.verdict = VERDICT_BUILD_FAILED
            self.status = "build-failed"
            self.red = True
            return
        # CPython baseline can't run -> no valid floor; not gated.
        if not self.cpython_ok:
            self.cpython_incompatible = True
            self.verdict = VERDICT_CPY_INCOMPAT
            self.status = "cpython-incompatible"
            self.red = False
            if self.note is None:
                self.note = "CPython baseline could not run this script standalone"
            return
        # CPython runs but molt does not -> a real molt run failure.
        if not self.molt_ok:
            self.verdict = VERDICT_RUN_ERROR
            self.status = "error"
            self.red = True
            if self.note is None:
                self.note = "molt run failed/unmeasurable while CPython ran"
            return

        # Ratios. cold/warm "ratio" == cpython/molt (legacy column names);
        # the council's "speedup" is the same quantity — we expose both names.
        self.cold_ratio = _safe_ratio(self.cold_cpython_s, self.cold_molt_s)
        self.warm_ratio = _safe_ratio(self.warm_cpython_s, self.warm_molt_s)
        self.cpython_ratio = self.warm_ratio
        self.warm_speedup = self.warm_ratio
        self.cold_speedup = self.cold_ratio

        # startup_tax_ms = the fixed cold-start cost molt pays that the warm
        # steady state does not (cold_total - warm_total). This is the quantity
        # the cold-start budget gates against — NOT the cold/cpython ratio.
        if self.cold_molt_s is not None and self.warm_molt_s is not None:
            self.startup_tax_ms = round(
                (self.cold_molt_s - self.warm_molt_s) * 1000.0, 2
            )

        # Unstable cell: cannot be trusted in EITHER direction -> gated.
        if not self.stable:
            self.verdict = VERDICT_UNSTABLE
            self.status = "unstable"
            self.red = True
            return

        warm_below = (
            self.warm_speedup is not None and self.warm_speedup <= RED_THRESHOLD
        )
        cold_below = (
            self.cold_speedup is not None and self.cold_speedup <= RED_THRESHOLD
        )
        over_budget = (
            budget_ms is not None
            and self.startup_tax_ms is not None
            and self.startup_tax_ms > budget_ms
        )

        # --- The two-dimensional decision (council ruling A) -------------
        # 1. FAIL_ENGINE — warm steady-state is at/below CPython. This is the
        #    execution-engine red, the release blocker. It dominates a cold
        #    failure (if the engine is slow, fix the engine first).
        if warm_below:
            self.verdict = VERDICT_FAIL_ENGINE
            self.status = "red"
            self.red = True
            self.suspected_missing_fact = self.suspected_missing_fact or _suspect_fact(
                self.benchmark
            )
            return
        # 2. FAIL_COLD_BUDGET — warm is fine but the fixed startup tax exceeds
        #    the recorded budget for this lane. A startup regression, not an
        #    engine red; routes to the cold-start lane.
        if over_budget:
            self.verdict = VERDICT_FAIL_COLD_BUDGET
            self.status = "red"
            self.red = True
            self.suspected_startup_component = (
                self.suspected_startup_component
                or _suspect_startup_component(self.benchmark)
            )
            return
        # 3. WARN_COLD_FLOOR — warm > CPython, but cold <= CPython AND the loss
        #    is solely the fixed startup tax (within budget). NOT an engine red;
        #    does not fail the gate unless --strict-cold.
        if cold_below:
            self.verdict = VERDICT_WARN_COLD_FLOOR
            self.status = "warn-cold"
            self.red = False  # not a hard red — fixed tax, within budget
            self.suspected_startup_component = (
                self.suspected_startup_component
                or _suspect_startup_component(self.benchmark)
            )
            return
        # 4. GREEN — warm fast, cold fast, within budget.
        self.verdict = VERDICT_GREEN
        self.status = "green"
        self.red = False


def _safe_ratio(numerator: float | None, denominator: float | None) -> float | None:
    if numerator is None or denominator is None or denominator <= 0:
        return None
    return round(numerator / denominator, 4)


# --- Triage hints (council "what FACT is missing from IR?" doctrine) --------
# These are NAME-pattern heuristics that point a perf agent at the most likely
# missing IR fact per the council triage doctrine. They are HINTS for the
# summary, never gating logic — the real diagnosis is the per-benchmark
# one-page representation analysis (ruling G).
_FACT_HINTS: tuple[tuple[tuple[str, ...], str], ...] = (
    (
        ("csv", "split", "field", "substr", "slice"),
        "substring/slice repr (alloc-free field extraction)",
    ),
    (
        ("exception", "raise", "try", "except"),
        "zero-cost happy-path exception-state + handler-region ownership",
    ),
    (
        ("etl", "orders", "record", "row"),
        "record/dict value-slot shape + borrow/ownership of stable field flow",
    ),
    (
        ("dict", "set", "map", "counter"),
        "hash-table value-slot Repr + key identity/borrow",
    ),
    (("tuple", "pack", "index"), "tuple element Repr + unboxed-lane stability"),
    (
        ("attr", "method", "dispatch", "class"),
        "class identity/method shape/version guard/call target (devirt)",
    ),
    (
        ("fib", "loop", "sum", "range", "sieve"),
        "induction/range/overflow/lane-stability (counted-loop facts)",
    ),
    (
        ("generator", "gen", "yield", "async", "await", "coro"),
        "frame ownership/resumable-state/fusion",
    ),
    (
        ("bytes", "bytearray", "str", "format"),
        "string/bytes Repr + borrowed-view extraction",
    ),
    (("json", "parse", "roundtrip"), "parse buffer ownership + value-slot Repr"),
)


def _suspect_fact(benchmark: str) -> str:
    name = Path(benchmark).stem.lower()
    for needles, fact in _FACT_HINTS:
        if any(n in name for n in needles):
            return fact
    return "representation/ownership fact (run the one-page diagnosis)"


def _suspect_startup_component(benchmark: str) -> str:
    """Cold-start tax components are program-INDEPENDENT for the fixed floor.

    The cold tax is dominated by the fixed startup cost (process-launch/dyld +
    molt-runtime-init + binary page-in), not the program. The per-benchmark
    delta is binary SIZE (larger linked surface = more page-in) and any extra
    module-init the program's imports trigger. See docs/perf/COLD_START.md.
    """
    name = Path(benchmark).stem.lower()
    if any(n in name for n in ("json", "csv", "import", "etl", "channel", "async")):
        return "module-init + binary page-in (program imports extra stdlib surface)"
    return "fixed startup floor: binary page-in + molt-runtime-init + dyld"


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
    budget_ms: float | None = None,
    authoritative: bool = True,
    pypy_bin: str | None = None,
    codon_runner: "CodonRunner | None" = None,
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
        cell.finalize(budget_ms=budget_ms, authoritative=authoritative)
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
        cell.finalize(budget_ms=budget_ms, authoritative=authoritative)
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

    # Stability: molt is the ARTIFACT UNDER TEST and MUST be stable. CPython is
    # the reference floor; a single CPython GC/scheduler outlier (common on
    # multi-100ms benchmarks) must NOT invalidate a cell where molt wins
    # decisively and is itself stable — otherwise a won class (e.g.
    # class_hierarchy 8x, molt cv 0.03) gets masked UNSTABLE by one CPython
    # spike. So the cell is trustworthy iff molt is stable AND either CPython is
    # stable OR the warm verdict is ROBUST to CPython's full sample spread (the
    # warm_speedup stays on the same side of the 1.00 floor using BOTH CPython's
    # fastest and slowest sample). This is median-based + outlier-robust per
    # pyperf discipline, never a per-test special case.
    cell.stable = _robust_cell_stable(molt_stats, cpy_stats)

    # One-time output parity (informational; not the gate).
    if cold_molt.stdout is not None and cold_cpy.stdout is not None:
        cell.output_parity = cold_molt.stdout.strip() == cold_cpy.stdout.strip()

    if not cell.molt_ok:
        cell.note = f"molt run unmeasurable (status={cold_molt.status})"
    elif not cell.cpython_ok:
        cell.note = f"cpython run unmeasurable (status={cold_cpy.status})"
    elif cell.stable and not cpy_stats.stable and molt_stats.stable:
        # Transparency: the cell is trusted despite CPython-side jitter because
        # molt is stable AND the verdict is robust to CPython's spread.
        cell.note = (
            f"cpython unstable (cv={cpy_stats.cv}) but verdict robust to its "
            f"spread; molt stable (cv={molt_stats.cv})"
        )

    # --- PyPy comparator lane (informational; never a hard gate) ------------
    # Only measured once per benchmark (lane-independent: PyPy runs the same
    # .py). We attach it to every backend cell for the same benchmark so the
    # column is populated, but it is a CPython-style interpreter comparator,
    # not a molt-backend fact.
    if pypy_bin is not None and cell.warm_molt_s:
        pypy_warm = _measure_interpreter_warm(
            pypy_bin,
            str(script_path),
            run_args,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"pypy:{benchmark}",
        )
        if pypy_warm is not None:
            cell.pypy_warm_s = round(pypy_warm, 6)
            cell.pypy_ratio = _safe_ratio(pypy_warm, cell.warm_molt_s)
            log_lines.append(
                f"pypy warm_median={cell.pypy_warm_s} ratio={cell.pypy_ratio}"
            )

    # --- Codon comparator lane (AOT north star; equivalence-gated) ----------
    if codon_runner is not None and cell.warm_molt_s:
        codon_runner.measure_into(
            cell,
            script_path=script_path,
            run_args=run_args,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            log_lines=log_lines,
        )

    cell.finalize(budget_ms=budget_ms, authoritative=authoritative)
    log_lines.append(
        f"molt warm_median={cell.warm_molt_s} cpython warm_median={cell.warm_cpython_s} "
        f"warm_speedup={cell.warm_speedup} cold_speedup={cell.cold_speedup} "
        f"startup_tax_ms={cell.startup_tax_ms} verdict={cell.verdict}"
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
# PyPy / Codon comparator lanes (council Lane C — best-effort, never a gate)
# ---------------------------------------------------------------------------
#
# PyPy: a DYNAMIC comparator (JIT interpreter). Where PyPy beats molt we NAME
# the missing molt fact; it is NOT a hard gate (PyPy's strength is long-running
# JIT warmup, a different operating point than molt's AOT).
#
# Codon: the AOT NORTH STAR — but Codon is NOT a drop-in for every Python
# program (no full CPython object model, restricted dynamism). A benchmark is
# scored against Codon ONLY if it is on the semantic-equivalence allowlist;
# everything else is recorded "non-equivalent" and NEVER scored win/loss.


def _measure_interpreter_warm(
    interp_bin: str,
    script: str,
    run_args: list[str],
    *,
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    label: str,
) -> float | None:
    """Warm steady-state median wall time for a Python-compatible interpreter.

    Same >=samples cold+warm discipline as the CPython path, through safe_run.
    Returns None if the interpreter cannot run the script (so a comparator that
    chokes on a benchmark simply leaves the column null, never poisons a cell).
    """
    env = _cpython_run_env()
    for _ in range(warmup):
        _safe_run_json(
            [interp_bin, script, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"{label}-warmup",
        )
    runs = [
        _safe_run_json(
            [interp_bin, script, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=label,
        )
        for _ in range(samples)
    ]
    stats = PhaseStats.from_runs(runs)
    return stats.median_s if stats.n > 0 else None


# Codon semantic-equivalence allowlist. ONLY these benchmarks are drop-in
# enough that a Codon AOT comparison is meaningful (numeric/loop kernels with
# no CPython-object-model dependence, no dynamic stdlib). A benchmark NOT in
# this set is recorded "non-equivalent" and never scored. Conservative by
# design — false "equivalent" is worse than a missing comparison.
CODON_EQUIVALENT_BENCHMARKS = frozenset(
    {
        "tests/benchmarks/bench_fib.py",
        "tests/benchmarks/bench_sieve.py",
        "tests/benchmarks/bench_sum_loop.py",
        "tests/benchmarks/bench_nbody.py",
        "tests/benchmarks/bench_mandelbrot.py",
        "tests/benchmarks/bench_spectral_norm.py",
        "tests/benchmarks/bench_binary_trees.py",
        "tests/benchmarks/bench_matrix_mul.py",
    }
)


class CodonRunner:
    """Compiles + times a benchmark with Codon when it is on the allowlist.

    Codon is AOT: we compile each allowlisted benchmark to a native binary
    (``codon build -release``) and time it through safe_run with the same
    discipline as molt. A compile failure or a not-allowlisted benchmark
    records the reason and marks the cell ``codon_equivalent`` accordingly —
    it is NEVER scored win/loss.
    """

    def __init__(self, codon_bin: str) -> None:
        self.codon_bin = codon_bin
        self._tmp_root: Path | None = None
        # Codon-compiled binaries link libomp/libcodonrt via @loader_path and
        # need the codon lib dir on DYLD_LIBRARY_PATH to run. Resolve it from
        # the binary location (.../bin/codon -> .../lib/codon).
        self.lib_dir = self._resolve_lib_dir(codon_bin)

    @staticmethod
    def _resolve_lib_dir(codon_bin: str) -> Path | None:
        root = Path(codon_bin).resolve().parent.parent  # .../bin/codon -> root
        cand = root / "lib" / "codon"
        return cand if cand.exists() else None

    def _run_env(self) -> dict[str, str]:
        env = _cpython_run_env()
        if self.lib_dir is not None:
            existing = env.get("DYLD_LIBRARY_PATH", "")
            env["DYLD_LIBRARY_PATH"] = (
                f"{self.lib_dir}:{existing}" if existing else str(self.lib_dir)
            )
            # macOS hardened-runtime strips DYLD_* across some exec boundaries;
            # CODON_LIBRARY is also honored by codon-compiled binaries.
            env.setdefault("CODON_LIBRARY", str(self.lib_dir))
        return env

    def _ensure_tmp(self) -> Path:
        if self._tmp_root is None:
            import tempfile

            self._tmp_root = Path(
                tempfile.mkdtemp(
                    prefix="perfscore-codon-", dir=str(bench.BENCH_TMP_ROOT)
                )
            )
        return self._tmp_root

    def measure_into(
        self,
        cell: "Cell",
        *,
        script_path: Path,
        run_args: list[str],
        samples: int,
        warmup: int,
        rss_mb: int,
        timeout_s: float,
        log_lines: list[str],
    ) -> None:
        key = bench_suites.canonical_benchmark_key(script_path)
        if key not in CODON_EQUIVALENT_BENCHMARKS:
            cell.codon_equivalent = False
            cell.codon_note = "non-equivalent (not on Codon drop-in allowlist)"
            log_lines.append("codon: non-equivalent (not allowlisted) — not scored")
            return
        cell.codon_equivalent = True
        out_bin = self._ensure_tmp() / f"{Path(key).stem}.codon"
        # Codon does not accept arbitrary argv the way CPython does; allowlisted
        # kernels are self-contained (no required run_args). If a benchmark
        # needs args, it should not be on the allowlist.
        build_cmd = [
            self.codon_bin,
            "build",
            "-release",
            "-o",
            str(out_bin),
            str(script_path),
        ]
        try:
            res = subprocess.run(
                build_cmd,
                capture_output=True,
                text=True,
                timeout=300,
                env=self._run_env(),
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            cell.codon_equivalent = False
            cell.codon_note = f"codon build error: {exc!r}"
            log_lines.append(f"codon build EXCEPTION: {exc!r} — not scored")
            return
        if res.returncode != 0 or not out_bin.exists():
            tail = (res.stderr or res.stdout or "").strip()[-400:]
            # A compile failure does NOT mean molt wins — it means the
            # comparison is unavailable. Record, do not score.
            cell.codon_equivalent = False
            cell.codon_note = f"codon build failed rc={res.returncode}: {tail}"
            log_lines.append(f"codon build FAILED rc={res.returncode} — not scored")
            return
        warm = _measure_codon_warm(
            str(out_bin),
            run_args,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"codon:{key}",
            env=self._run_env(),
        )
        if warm is None:
            cell.codon_note = "codon run unmeasurable"
            log_lines.append("codon run unmeasurable — not scored")
            return
        cell.codon_warm_s = round(warm, 6)
        cell.codon_ratio = _safe_ratio(warm, cell.warm_molt_s)
        cell.codon_note = "equivalent (codon -release AOT)"
        log_lines.append(
            f"codon warm_median={cell.codon_warm_s} ratio={cell.codon_ratio}"
        )

    def close(self) -> None:
        if self._tmp_root is not None:
            import shutil

            shutil.rmtree(self._tmp_root, ignore_errors=True)
            self._tmp_root = None


def _measure_codon_warm(
    codon_bin_path: str,
    run_args: list[str],
    *,
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    label: str,
    env: dict[str, str] | None = None,
) -> float | None:
    """Warm median wall time of a compiled Codon binary (safe_run-guarded)."""
    if env is None:
        env = _cpython_run_env()
    for _ in range(warmup):
        _safe_run_json(
            [codon_bin_path, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=f"{label}-warmup",
        )
    runs = [
        _safe_run_json(
            [codon_bin_path, *run_args],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label=label,
        )
        for _ in range(samples)
    ]
    stats = PhaseStats.from_runs(runs)
    return stats.median_s if stats.n > 0 else None


# ---------------------------------------------------------------------------
# Provenance — the anti-stale-lore enforcement (council ruling A + B)
# ---------------------------------------------------------------------------
#
# Every emitted board carries the exact tree + tool + artifact identity it was
# measured against. If the local HEAD diverges from origin/main the board is
# stamped non-authoritative and the gate refuses (FAIL_STALE) unless the caller
# explicitly opts into local debugging with --allow-nonauthoritative. This is
# the mechanical kill for the "rediscovered a stale-tree failure" class.


def _git_rev() -> str:
    return bench._git_rev() or "unknown"


def _git_output(args: list[str]) -> str | None:
    try:
        res = subprocess.run(
            ["git", *args],
            cwd=str(REPO_ROOT),
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if res.returncode != 0:
        return None
    out = res.stdout.strip()
    return out or None


def _origin_main_sha() -> str | None:
    """SHA of origin/main as the local remote-tracking ref knows it.

    We do NOT fetch here (that is a network side effect the caller controls);
    we report the ref the working tree already has. The Lane-C contract is to
    run inside a worktree freshly checked out at origin/main, so this is the
    authoritative remote tip for the run.
    """
    return _git_output(["rev-parse", "origin/main"])


def _benchmark_tool_identity() -> dict[str, str | None]:
    """Identity of perf_scoreboard.py itself (its own git blob + last commit).

    A board measured by a modified-but-uncommitted tool is as non-authoritative
    as a board measured against a modified tree; we surface both so the reader
    can tell whether the SCOREBOARD LOGIC changed, not just the compiler.
    """
    rel = str(Path(__file__).resolve().relative_to(REPO_ROOT))
    blob = _git_output(["hash-object", str(Path(__file__).resolve())])
    last_commit = _git_output(["log", "-n", "1", "--format=%H", "--", rel])
    # Does the committed blob differ from the on-disk file?
    head_blob = _git_output(["rev-parse", f"HEAD:{rel}"])
    return {
        "path": rel,
        "ondisk_blob_sha": blob,
        "head_blob_sha": head_blob,
        "last_commit_sha": last_commit,
        "modified_vs_head": str(blob is not None and blob != head_blob).lower(),
    }


def _backend_binary_identity_for(spec: "BackendSpec", profile: str) -> str | None:
    """Reuse cli.py's ``_backend_binary_identity`` for the daemon backend binary.

    The backend binary identity (path|mtime_ns|size) is the SAME signal the
    stdlib/TIR caches salt their namespace with; recording it on the board lets
    a reader prove the artifact a number was measured against, and detect the
    stale-cache confound class directly. If cli.py cannot be imported (it is a
    heavy module) we degrade to None rather than fail the board.
    """
    try:
        import molt.cli as _cli  # noqa: PLC0415 - optional, heavy import
    except Exception:  # noqa: BLE001
        return None
    fn = getattr(_cli, "_backend_binary_identity", None)
    if fn is None:
        return None
    backend_bin = _resolve_backend_binary_path(spec, profile)
    if backend_bin is None:
        return None
    try:
        return fn(backend_bin)
    except Exception:  # noqa: BLE001
        return None


def _resolve_backend_binary_path(spec: "BackendSpec", profile: str) -> Path | None:
    """Best-effort path to the daemon's molt-backend binary for this lane.

    The daemon backend is the cargo ``molt-backend`` artifact. Its location
    depends on the active CARGO_TARGET_DIR: it may be the shared
    ``target/<profile_dir>/`` (solo-dev / when CARGO_TARGET_DIR points at
    ``target``) or a session dir ``target/sessions/<id>/<profile_dir>/``. We
    probe, in order: the live ``CARGO_TARGET_DIR``, the perfscore session dir,
    and the shared ``target/`` root — covering every layout cli.py uses.
    release-fast/release-output map to the ``release-fast`` cargo profile dir;
    dev maps to ``debug``. Returns None if nothing is found (the identity then
    degrades to None, never crashes).
    """
    profile_dir = (
        "release-fast" if PROFILE_BUILD_FLAG.get(profile) == "release" else "debug"
    )
    roots: list[Path] = []
    env_target = os.environ.get("CARGO_TARGET_DIR", "").strip()
    if env_target:
        roots.append(Path(env_target))
    roots.append(REPO_ROOT / "target" / "sessions" / PERFSCORE_SESSION_ID)
    roots.append(REPO_ROOT / "target")
    seen: set[Path] = set()
    for root in roots:
        if root in seen:
            continue
        seen.add(root)
        for name in ("molt-backend", "molt"):
            cand = root / profile_dir / name
            if cand.exists():
                return cand
    return None


def _stdlib_cache_key_signal() -> str | None:
    """A stable signal for the stdlib/runtime cache identity.

    The full per-build ``_shared_stdlib_cache_key`` needs the program IR (only
    available mid-build), so for board-level provenance we record the runtime
    backend source-tree fingerprint (``_cache_fingerprint``) which is exactly
    the ``runtime_backend`` component that key is salted with. A reader can
    diff this across boards to know whether the runtime/codegen sources moved.
    """
    try:
        import molt.cli as _cli  # noqa: PLC0415

        fn = getattr(_cli, "_cache_fingerprint", None)
        if fn is not None:
            return str(fn())
    except Exception:  # noqa: BLE001
        return None
    return None


def gather_provenance(
    specs_profiles: list[tuple["BackendSpec", str]] | None = None,
) -> dict:
    """Collect the full provenance metadata for a board (council ruling A).

    ``authoritative`` is False whenever the local HEAD diverges from origin/main
    OR the tree is dirty OR the scoreboard tool itself is modified-vs-HEAD —
    any of which means the numbers are not the canonical origin/main contract.
    """
    local_head = _git_output(["rev-parse", "HEAD"])
    origin = _origin_main_sha()
    merge_base = (
        _git_output(["merge-base", "HEAD", "origin/main"])
        if origin is not None
        else None
    )
    dirty = bool(_git_output(["status", "--porcelain"]))
    tool = _benchmark_tool_identity()
    tool_modified = tool.get("modified_vs_head") == "true"

    diverges = bool(local_head and origin and local_head != origin)
    authoritative = not (diverges or dirty or tool_modified)

    backend_identities: dict[str, str | None] = {}
    if specs_profiles:
        for spec, profile in specs_profiles:
            ident = _backend_binary_identity_for(spec, profile)
            backend_identities[f"{spec.backend}/{profile}"] = ident

    return {
        "origin_sha": origin,
        "local_head_sha": local_head,
        "merge_base_sha": merge_base,
        "dirty_tree": dirty,
        "diverges_from_origin": diverges,
        "benchmark_tool_sha": tool.get("ondisk_blob_sha"),
        "benchmark_tool_last_commit": tool.get("last_commit_sha"),
        "benchmark_tool_modified": tool_modified,
        "backend_binary_identity": backend_identities,
        "stdlib_cache_key": _stdlib_cache_key_signal(),
        "authoritative": authoritative,
        "authoritative_reason": _authoritative_reason(diverges, dirty, tool_modified),
    }


def _authoritative_reason(diverges: bool, dirty: bool, tool_modified: bool) -> str:
    if not (diverges or dirty or tool_modified):
        return "tree == origin/main, clean, tool unmodified"
    parts = []
    if diverges:
        parts.append("local HEAD diverges from origin/main")
    if dirty:
        parts.append("working tree is dirty")
    if tool_modified:
        parts.append("perf_scoreboard.py modified vs HEAD")
    return "; ".join(parts)


def build_scoreboard_doc(
    cells: list[Cell],
    *,
    benchmarks_run: list[str],
    benchmarks_deferred: list[dict],
    cpython_version: str,
    samples: int,
    warmup: int,
    provenance: dict | None = None,
    pypy_version: str | None = None,
    codon_version: str | None = None,
) -> dict:
    """Assemble the nested machine-readable scoreboard (schema 3).

    Shape: ``benchmark -> target -> backend -> profile -> {cell fields}``.
    Adds the two-dimensional verdict breakdown + the provenance block; keeps
    every legacy field so schema-2 readers still work.
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

    def keys_with(verdict: str) -> list[str]:
        return sorted(_cell_key(asdict(c)) for c in cells if c.verdict == verdict)

    # The gate-failing set (the hard reds). FAIL_STALE is conditional (depends
    # on --allow-nonauthoritative), so it is reported separately, not summed in.
    gate_failing = [c for c in cells if c.verdict in GATE_FAILING_VERDICTS]
    stale_cells = [c for c in cells if c.verdict == VERDICT_FAIL_STALE]
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "cpython_floor_scoreboard",
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": git_rev,
        "provenance": provenance or {},
        "host": {
            "platform": sys.platform,
            "python_runner": sys.version.split()[0],
            "cpython_baseline": cpython_version,
            "pypy": pypy_version,
            "codon": codon_version,
        },
        "direction": "speedup = cpython_time / molt_time; >1.0 = molt faster; <=1.0 = engine/cold red",
        "red_threshold": RED_THRESHOLD,
        "unstable_cv_threshold": UNSTABLE_CV,
        "verdict_legend": {
            "GREEN": "warm fast, cold fast, within cold-start budget",
            "FAIL_ENGINE": "warm_speedup <= 1.00 — execution-engine red, RELEASE BLOCKER",
            "FAIL_COLD_BUDGET": "startup_tax_ms > budget_ms — startup regression",
            "WARN_COLD_FLOOR": "cold <= 1.00 but warm > 1.00 & tax within budget — not a hard red",
            "FAIL_STALE": "non-authoritative tree — overrides all (gate fails unless --allow-nonauthoritative)",
            "UNSTABLE": "CV above threshold — untrustworthy in either direction",
            "BUILD_FAILED": "molt build failed",
            "RUN_ERROR": "CPython ran but molt did not",
            "RUN_BLOCKED": "wasm run-path gap (build/link only)",
            "CPY_INCOMPATIBLE": "no CPython floor — excluded from gate",
        },
        "methodology": {
            "samples_per_phase": samples,
            "warmup_runs": warmup,
            "cold_and_warm": True,
            "run_guard": "tools/safe_run.py --json (rss cap + timeout + peak rss)",
            "build": "tools/bench.py daemon batch build (memory-guarded)",
            "warm_speedup": "cpython_warm / molt_warm",
            "cold_speedup": "cpython_cold / molt_cold",
            "startup_tax_ms": "(molt_cold_total - molt_warm_total) * 1000",
        },
        "reserved_columns": {
            "pypy_ratio": (
                f"PyPy {pypy_version}: pypy_warm/molt_warm (>1 = molt faster)"
                if pypy_version
                else "nullable — PyPy not installed"
            ),
            "codon_ratio": (
                f"Codon {codon_version}: codon_warm/molt_warm, equivalence-gated"
                if codon_version
                else "nullable — Codon not installed"
            ),
        },
        "summary": {
            "cells_total": len(cells),
            "cells_green": sum(1 for c in cells if c.verdict == VERDICT_GREEN),
            # cells_red = the legacy "anything gated" count (back-compat).
            "cells_red": sum(1 for c in cells if c.red),
            "cells_unstable": sum(1 for c in cells if c.verdict == VERDICT_UNSTABLE),
            "cells_build_failed": sum(
                1 for c in cells if c.verdict == VERDICT_BUILD_FAILED
            ),
            "cells_run_blocked": sum(
                1 for c in cells if c.verdict == VERDICT_RUN_BLOCKED
            ),
            "cells_error": sum(1 for c in cells if c.verdict == VERDICT_RUN_ERROR),
            "cells_cpython_incompatible": sum(
                1 for c in cells if c.verdict == VERDICT_CPY_INCOMPAT
            ),
            # The two-dimensional verdict counts (the council's gate axes).
            "cells_fail_engine": sum(
                1 for c in cells if c.verdict == VERDICT_FAIL_ENGINE
            ),
            "cells_fail_cold_budget": sum(
                1 for c in cells if c.verdict == VERDICT_FAIL_COLD_BUDGET
            ),
            "cells_warn_cold_floor": sum(
                1 for c in cells if c.verdict == VERDICT_WARN_COLD_FLOOR
            ),
            "cells_fail_stale": len(stale_cells),
            "any_red": bool(gate_failing),
            "gate_fails": bool(gate_failing),
            # The two-dimensional breakdown (council ruling A) — every cell
            # keyed by its verdict so a reader routes warm reds to the IR-fact
            # lane and cold reds to the startup lane WITHOUT re-deriving them.
            "verdict_breakdown": {
                "FAIL_ENGINE": keys_with(VERDICT_FAIL_ENGINE),
                "FAIL_COLD_BUDGET": keys_with(VERDICT_FAIL_COLD_BUDGET),
                "WARN_COLD_FLOOR": keys_with(VERDICT_WARN_COLD_FLOOR),
                "FAIL_STALE": keys_with(VERDICT_FAIL_STALE),
                "UNSTABLE": keys_with(VERDICT_UNSTABLE),
                "BUILD_FAILED": keys_with(VERDICT_BUILD_FAILED),
                "RUN_ERROR": keys_with(VERDICT_RUN_ERROR),
                "CPY_INCOMPATIBLE": keys_with(VERDICT_CPY_INCOMPAT),
                "GREEN": keys_with(VERDICT_GREEN),
            },
            # Legacy alias kept for any downstream that still reads it.
            "red_breakdown": {
                "warm_red": keys_with(VERDICT_FAIL_ENGINE),
                "cold_only_red": keys_with(VERDICT_FAIL_COLD_BUDGET)
                + keys_with(VERDICT_WARN_COLD_FLOOR),
                "unstable": keys_with(VERDICT_UNSTABLE),
                "build_failed_or_error": keys_with(VERDICT_BUILD_FAILED)
                + keys_with(VERDICT_RUN_ERROR),
                "cpython_incompatible": keys_with(VERDICT_CPY_INCOMPAT),
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
        "provenance",
        "host",
        "direction",
        "red_threshold",
        "verdict_legend",
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
    # Provenance block must carry the council-mandated identity fields.
    required_prov = {
        "origin_sha",
        "local_head_sha",
        "merge_base_sha",
        "dirty_tree",
        "benchmark_tool_sha",
        "backend_binary_identity",
        "stdlib_cache_key",
        "authoritative",
    }
    pmiss = required_prov - set(doc.get("provenance", {}))
    if pmiss:
        problems.append(f"provenance missing fields: {sorted(pmiss)}")
    # Summary must carry the two-dimensional verdict counts + breakdown.
    required_summary = {
        "cells_fail_engine",
        "cells_fail_cold_budget",
        "cells_warn_cold_floor",
        "cells_fail_stale",
        "verdict_breakdown",
        "gate_fails",
    }
    smiss = required_summary - set(doc.get("summary", {}))
    if smiss:
        problems.append(f"summary missing 2-D fields: {sorted(smiss)}")
    # JSON round-trips.
    try:
        json.loads(json.dumps(doc))
    except (TypeError, ValueError) as exc:
        problems.append(f"doc is not JSON-serializable: {exc}")
    # Every cell must carry the verdict + 2-D measurement fields. pypy/codon
    # are now POPULATED when those toolchains are installed (no longer asserted
    # null) — only their presence as keys is required.
    required_cell = {
        "benchmark",
        "target",
        "backend",
        "profile",
        "cpython_ratio",
        "cold_ratio",
        "warm_ratio",
        "warm_speedup",
        "cold_speedup",
        "startup_tax_ms",
        "verdict",
        "binary_size_kib",
        "molt_peak_rss_mib",
        "compile_time_s",
        "stable",
        "red",
        "status",
        "pypy_ratio",
        "codon_ratio",
        "codon_equivalent",
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
        if c.get("verdict") in (None, "pending"):
            problems.append(
                f"cell {c.get('benchmark')} has unfinalized verdict {c.get('verdict')!r}"
            )
            break
    return problems


# ---------------------------------------------------------------------------
# Human-readable summary (RED rows first)
# ---------------------------------------------------------------------------


def print_summary(doc: dict) -> None:
    cells = _flatten_cells(doc)
    by_verdict: dict[str, list[dict]] = {}
    for c in cells:
        by_verdict.setdefault(c.get("verdict", "pending"), []).append(c)

    # --- Authoritative-tree header (council ruling A + B) -------------------
    prov = doc.get("provenance", {})
    authoritative = prov.get("authoritative", True)
    print("\n" + "=" * 100)
    print("CPYTHON FLOOR SCOREBOARD — two-dimensional (warm ≠ cold)")
    print(
        f"  origin/main = {_short(prov.get('origin_sha'))}   "
        f"local HEAD = {_short(prov.get('local_head_sha'))}   "
        f"tool = {_short(prov.get('benchmark_tool_sha'))}"
    )
    print(
        f"  cpython={doc['host']['cpython_baseline']}  "
        f"pypy={doc['host'].get('pypy') or '-'}  "
        f"codon={doc['host'].get('codon') or '-'}"
    )
    if authoritative:
        print("  AUTHORITATIVE: tree == origin/main, clean, tool unmodified")
    else:
        print(
            "  *** WARNING: local tree diverges from origin; benchmark is "
            "non-authoritative ***"
        )
        print(f"      reason: {prov.get('authoritative_reason', 'unknown')}")
    print("=" * 100)

    # --- Full table (verdict-ordered) --------------------------------------
    rank = {
        VERDICT_FAIL_STALE: 0,
        VERDICT_FAIL_ENGINE: 1,
        VERDICT_BUILD_FAILED: 1,
        VERDICT_RUN_ERROR: 1,
        VERDICT_UNSTABLE: 2,
        VERDICT_FAIL_COLD_BUDGET: 3,
        VERDICT_WARN_COLD_FLOOR: 4,
        VERDICT_RUN_BLOCKED: 5,
        VERDICT_CPY_INCOMPAT: 5,
        VERDICT_GREEN: 6,
    }
    cells.sort(
        key=lambda c: (
            rank.get(c.get("verdict"), 7),
            -(c.get("warm_speedup") or c.get("cpython_ratio") or 0.0),
            c["benchmark"],
        )
    )
    hdr = (
        f"{'VERDICT':<17}{'WARM':>7}  {'COLD':>7}  {'TAXms':>7}  "
        f"{'PYPY':>6}  {'CODON':>6}  {'SIZEKiB':>8}  BENCHMARK [backend/profile]"
    )
    print(hdr)
    print("-" * 100)
    for c in cells:
        print(
            f"{c.get('verdict', '?'):<17}"
            f"{_fmt(c.get('warm_speedup')):>7}  "
            f"{_fmt(c.get('cold_speedup')):>7}  "
            f"{_fmt(c.get('startup_tax_ms'), 0):>7}  "
            f"{_fmt(c.get('pypy_ratio')):>6}  "
            f"{_fmt(c.get('codon_ratio')):>6}  "
            f"{_fmt(c.get('binary_size_kib'), 0):>8}  "
            f"{c['benchmark']} [{c['backend']}/{c['profile']}]"
        )
    print("-" * 100)

    s = doc["summary"]
    print(
        f"TOTAL={s['cells_total']}  GREEN={s['cells_green']}  "
        f"FAIL_ENGINE={s.get('cells_fail_engine', 0)}  "
        f"FAIL_COLD_BUDGET={s.get('cells_fail_cold_budget', 0)}  "
        f"WARN_COLD_FLOOR={s.get('cells_warn_cold_floor', 0)}  "
        f"UNSTABLE={s['cells_unstable']}  BUILD_FAIL={s['cells_build_failed']}  "
        f"RUN_ERROR={s['cells_error']}  CPY_INCOMPAT={s.get('cells_cpython_incompatible', 0)}  "
        f"STALE={s.get('cells_fail_stale', 0)}"
    )

    # --- WARM EXECUTION REDS (the release blockers — IR-fact lane) ---------
    warm_reds = by_verdict.get(VERDICT_FAIL_ENGINE, [])
    print(
        f"\nWARM EXECUTION REDS ({len(warm_reds)}) — execution-engine, RELEASE BLOCKER:"
    )
    print("  (needs an IR FACT, not a local opt — see council triage doctrine)")
    for c in sorted(warm_reds, key=lambda c: c.get("warm_speedup") or 0.0):
        print(
            f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]  -> {c.get('suspected_missing_fact', '?')}"
        )

    # --- COLD-START BUDGET REDS (startup/runtime/artifact lane) -------------
    cold_reds = by_verdict.get(VERDICT_FAIL_COLD_BUDGET, [])
    print(f"\nCOLD-START BUDGET REDS ({len(cold_reds)}) — startup tax over budget:")
    for c in sorted(cold_reds, key=lambda c: -(c.get("startup_tax_ms") or 0.0)):
        print(
            f"    cold={_fmt(c.get('cold_speedup'))}x  tax={_fmt(c.get('startup_tax_ms'), 0)}ms"
            f" (budget {_fmt(c.get('cold_budget_ms'), 0)}ms)  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]  -> {c.get('suspected_startup_component', '?')}"
        )

    # --- WARN_COLD_FLOOR (cold<=1 but warm>1, tax within budget) -----------
    warn_cold = by_verdict.get(VERDICT_WARN_COLD_FLOOR, [])
    if warn_cold:
        print(
            f"\nCOLD-FLOOR WARNINGS ({len(warn_cold)}) — warm>CPython, cold<=CPython "
            "by FIXED startup tax (within budget; NOT a gate fail unless --strict-cold):"
        )
        for c in sorted(warn_cold, key=lambda c: -(c.get("startup_tax_ms") or 0.0))[
            :12
        ]:
            print(
                f"    cold={_fmt(c.get('cold_speedup'))}x  warm={_fmt(c.get('warm_speedup'))}x"
                f"  tax={_fmt(c.get('startup_tax_ms'), 0)}ms  {c['benchmark']} "
                f"[{c['backend']}/{c['profile']}]"
            )
        if len(warn_cold) > 12:
            print(
                f"    ... and {len(warn_cold) - 12} more (full list in JSON verdict_breakdown)"
            )

    # --- BACKEND ERRORS / NON-AUTHORITATIVE --------------------------------
    errs = (
        by_verdict.get(VERDICT_BUILD_FAILED, [])
        + by_verdict.get(VERDICT_RUN_ERROR, [])
        + by_verdict.get(VERDICT_UNSTABLE, [])
    )
    stale = by_verdict.get(VERDICT_FAIL_STALE, [])
    if errs or stale:
        print(f"\nBACKEND ERRORS / NON-AUTHORITATIVE ({len(errs) + len(stale)}):")
        for c in errs:
            origin_rerun = "yes" if not authoritative else "no"
            print(
                f"    {c.get('verdict'):<16} {c['benchmark']} [{c['backend']}/{c['profile']}]"
                f"  stale?={'yes' if not authoritative else 'no'}  "
                f"origin_rerun_needed?={origin_rerun}"
                + (f"  ({c.get('note')})" if c.get("note") else "")
            )
        for c in stale[:5]:
            print(
                f"    FAIL_STALE       {c['benchmark']} [{c['backend']}/{c['profile']}]"
                "  stale?=yes  origin_rerun_needed?=yes"
            )
        if len(stale) > 5:
            print(
                f"    ... and {len(stale) - 5} more stale cells (whole board non-authoritative)"
            )

    # --- REGRESSIONS FROM LAST GREEN (filled by the baseline-diff caller) ---
    regressions = doc.get("_regressions_from_last_green")
    if regressions:
        print(f"\nREGRESSIONS FROM LAST GREEN ({len(regressions)}):")
        for m in regressions:
            print(f"    {m}")

    # --- GREENS WORTH PROTECTING (>2x — do not reopen a won class) ----------
    greens = by_verdict.get(VERDICT_GREEN, [])
    protected = sorted(
        (c for c in greens if (c.get("warm_speedup") or 0.0) > 2.0),
        key=lambda c: -(c.get("warm_speedup") or 0.0),
    )
    print(f"\nGREENS WORTH PROTECTING ({len(protected)}) — won classes, do NOT reopen:")
    for c in protected:
        print(
            f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]"
        )

    # --- FASTEST NEXT UNLOCK -----------------------------------------------
    unlock = _fastest_next_unlock(warm_reds, cold_reds)
    print(f"\nFASTEST NEXT UNLOCK: {unlock}")

    if doc["benchmarks_deferred"]:
        print(f"\nDEFERRED / CPY-INCOMPATIBLE ({len(doc['benchmarks_deferred'])}):")
        for d in doc["benchmarks_deferred"][:8]:
            print(f"  - {d['benchmark']}: {d['reason']}")
        if len(doc["benchmarks_deferred"]) > 8:
            print(f"  ... and {len(doc['benchmarks_deferred']) - 8} more")
    print("=" * 100 + "\n")


def _short(sha: str | None) -> str:
    if not sha:
        return "-"
    return sha[:12]


def _fastest_next_unlock(warm_reds: list[dict], cold_reds: list[dict]) -> str:
    """One structural fact / one file lane / one gate — the highest-leverage next move.

    Prefer the WORST warm red (engine reds outrank cold reds per ruling A); a
    warm red the most benchmarks share is the fastest class to retire.
    """
    if warm_reds:
        worst = min(warm_reds, key=lambda c: c.get("warm_speedup") or 1e9)
        return (
            f"heal {worst['benchmark']} [{worst['backend']}/{worst['profile']}] "
            f"({_fmt(worst.get('warm_speedup'))}x) — fact: "
            f"{worst.get('suspected_missing_fact', '?')}"
        )
    if cold_reds:
        worst = max(cold_reds, key=lambda c: c.get("startup_tax_ms") or 0.0)
        return (
            f"cold-start: {worst['benchmark']} tax={_fmt(worst.get('startup_tax_ms'), 0)}ms "
            f"— attack {worst.get('suspected_startup_component', '?')}"
        )
    return "no reds — protect the greens; widen the suite for the next class"


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
        # Prefer the warm_speedup axis (the engine fact); fall back to the
        # legacy cpython_ratio for boards measured by the schema-2 tool.
        new_ratio = c.get("warm_speedup") or c.get("cpython_ratio")
        old_ratio = old.get("warm_speedup") or old.get("cpython_ratio")
        # "Newly gating" = a green-or-warn cell that became a hard gate fail.
        was_green = not old.get("red") or old.get("verdict") == VERDICT_WARN_COLD_FLOOR
        now_fails = c.get("verdict") in GATE_FAILING_VERDICTS
        if now_fails and was_green:
            newly_red.append(
                f"{key}: NEWLY {c.get('verdict', 'RED')}  "
                f"{_fmt(old_ratio)} -> {_fmt(new_ratio)}"
            )
        elif (
            new_ratio is not None
            and old_ratio is not None
            and not now_fails
            and new_ratio < old_ratio * 0.95  # >5% slower but still passing
        ):
            regressed.append(
                f"{key}: regressed-but-passing  {_fmt(old_ratio)} -> {_fmt(new_ratio)} "
                f"({(new_ratio / old_ratio - 1) * 100:+.1f}%)"
            )
    return newly_red, regressed


def _cell_key(c: dict) -> str:
    return f"{c['benchmark']} [{c['backend']}/{c['profile']}]"


def _latest_baseline(exclude: Path | None = None) -> Path | None:
    """The most recent committed board, EXCLUDING in-progress ``.partial.json``
    checkpoints and an optional explicit path (the board being written now).

    Without the ``.partial`` exclusion a diff would compare a board against its
    own mid-sweep checkpoint (a near-self-diff that hides every regression).
    """
    if not SCOREBOARD_DIR.exists():
        return None
    exclude_resolved = exclude.resolve() if exclude is not None else None
    candidates = [
        p
        for p in sorted(SCOREBOARD_DIR.glob("cpython_*.json"))
        if not p.name.endswith(".partial.json")
        and (exclude_resolved is None or p.resolve() != exclude_resolved)
    ]
    return candidates[-1] if candidates else None


def _gate_exit_code(
    doc: dict,
    *,
    no_gate: bool,
    strict_cold: bool = False,
    allow_nonauthoritative: bool = False,
) -> int:
    """The two-dimensional gate (council ruling A).

    Nonzero iff any FAIL_ENGINE / FAIL_COLD_BUDGET / BUILD_FAILED / RUN_ERROR /
    UNSTABLE. WARN_COLD_FLOOR fails ONLY with ``--strict-cold``. FAIL_STALE
    fails UNLESS ``--allow-nonauthoritative`` (local-debug opt-out). The single
    source of truth shared by run / merge / rebuild-summary.
    """
    if no_gate:
        return 0
    s = doc.get("summary", {})
    if s.get("gate_fails"):
        return 1
    if strict_cold and s.get("cells_warn_cold_floor", 0) > 0:
        return 1
    if (not allow_nonauthoritative) and s.get("cells_fail_stale", 0) > 0:
        return 1
    return 0


def _finalize_with_board_context(cells: list[Cell], doc_like: dict) -> None:
    """Re-finalize stored cells using budgets + the board's own authoritative flag.

    For rebuild/merge we re-run the classifier so a stored board reflects the
    CURRENT verdict logic. The cold-start budget comes from the live budget
    file; the authoritative flag comes from the stored provenance (a stored
    board does not re-derive authoritativeness — it was already stamped). We
    also RE-DERIVE ``stable`` from the stored per-runtime stats so a board
    measured by an older tool picks up the current robust-stability rule (the
    CPython-outlier tolerance) without re-running any benchmark.
    """
    budgets = _load_cold_start_budgets()
    authoritative = doc_like.get("provenance", {}).get("authoritative", True)
    for cell in cells:
        _rederive_stability(cell)
        cell.finalize(
            budget_ms=_budget_ms_for(budgets, cell.backend, cell.profile),
            authoritative=authoritative,
        )


def _rederive_stability(cell: Cell) -> None:
    """Recompute ``cell.stable`` from the stored molt/cpython stats dicts.

    ``finalize`` does not recompute stability (it is set at measurement time),
    so a rebuild-summary/merge must re-derive it to apply the current robust
    rule. No-op if the stored stats are absent.
    """
    if not cell.molt_stats or not cell.cpython_stats:
        return
    molt = _phasestats_from_dict(cell.molt_stats)
    cpy = _phasestats_from_dict(cell.cpython_stats)
    if molt is None or cpy is None:
        return
    cell.stable = _robust_cell_stable(molt, cpy)


def _phasestats_from_dict(d: dict) -> PhaseStats | None:
    import dataclasses

    if not isinstance(d, dict):
        return None
    known = {f.name for f in dataclasses.fields(PhaseStats)}
    return PhaseStats(**{k: v for k, v in d.items() if k in known})


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


def _rebuild_summary(
    path: Path,
    *,
    no_gate: bool,
    strict_cold: bool = False,
    allow_nonauthoritative: bool = False,
) -> int:
    """Re-derive a stored board's summary/breakdown/gate from its per-cell data.

    Loads the authoritative per-cell measurements, re-runs them through the
    CURRENT ``build_scoreboard_doc`` (so the summary + verdicts match the
    current tool), writes the board back in place, prints the summary, and
    returns the gate exit code. No binaries are rebuilt; no benchmarks re-run.
    """
    try:
        prior = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"--rebuild-summary: cannot read {path}: {exc}", file=sys.stderr)
        return 2
    cells = [_cell_from_dict(c) for c in _flatten_cells(prior)]
    # Re-run the classifier on the stored measurements so the verdict reflects
    # the CURRENT finalize() logic (the 2-D verdict + budget), not whatever the
    # board was stamped with at measurement time.
    _finalize_with_board_context(cells, prior)
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
    host = prior.get("host", {})
    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=prior.get("benchmarks_run", []),
        benchmarks_deferred=deferred,
        cpython_version=host.get("cpython_baseline", "unknown"),
        samples=method.get("samples_per_phase", DEFAULT_SAMPLES),
        warmup=method.get("warmup_runs", DEFAULT_WARMUP),
        provenance=prior.get("provenance", {}),
        pypy_version=host.get("pypy"),
        codon_version=host.get("codon"),
    )
    # Preserve the original generation timestamp + git_rev of the measurement.
    doc["generated_at"] = prior.get("generated_at", doc["generated_at"])
    doc["git_rev"] = prior.get("git_rev", doc["git_rev"])
    if "host" in prior:
        doc["host"] = prior["host"]
    path.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    print(f"[rebuild-summary] rewrote {path}", file=sys.stderr)
    print_summary(doc)
    return _gate_exit_code(
        doc,
        no_gate=no_gate,
        strict_cold=strict_cold,
        allow_nonauthoritative=allow_nonauthoritative,
    )


def _merge_boards(
    sources: list[Path],
    out: Path,
    *,
    no_gate: bool,
    strict_cold: bool = False,
    allow_nonauthoritative: bool = False,
) -> int:
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
    provenance: dict = {}
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
        provenance = doc.get("provenance", provenance)
        cpython_version = host.get("cpython_baseline", cpython_version)
        git_rev = doc.get("git_rev", git_rev)
        generated_at = doc.get("generated_at", generated_at)
        for d in _flatten_cells(doc):
            cell = _cell_from_dict(d)
            by_key[(cell.benchmark, cell.target, cell.backend, cell.profile)] = cell
        for b in doc.get("benchmarks_run", []):
            if b not in benchmarks_run:
                benchmarks_run.append(b)
    cells = list(by_key.values())
    _finalize_with_board_context(cells, {"provenance": provenance})
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
        provenance=provenance,
        pypy_version=host.get("pypy"),
        codon_version=host.get("codon"),
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
    return _gate_exit_code(
        doc,
        no_gate=no_gate,
        strict_cold=strict_cold,
        allow_nonauthoritative=allow_nonauthoritative,
    )


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
        "--strict-cold",
        action="store_true",
        help="make WARN_COLD_FLOOR fail the gate too (default: cold-floor warns only)",
    )
    parser.add_argument(
        "--allow-nonauthoritative",
        action="store_true",
        help=(
            "permit a non-authoritative tree (local != origin/main, or dirty) to "
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
        return _rebuild_summary(
            Path(ns.rebuild_summary),
            no_gate=ns.no_gate,
            strict_cold=ns.strict_cold,
            allow_nonauthoritative=ns.allow_nonauthoritative,
        )

    if ns.merge is not None:
        merge_out = (
            Path(ns.out) if ns.out else SCOREBOARD_DIR / f"cpython_{_git_rev()}.json"
        )
        return _merge_boards(
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

    scripts = _resolve_benchmark_set(ns.set, ns.benchmark)
    cpython_bin = _resolve_system_cpython(ns.cpython)
    cpython_version = _probe_cpython_version(cpython_bin)
    print(
        f"[scoreboard] CPython oracle: {cpython_bin} ({cpython_version})",
        file=sys.stderr,
    )

    # --- Provenance + authoritative gate (council ruling A + B) ------------
    specs_profiles = [(BACKENDS_BY_NAME[b], p) for b in backends for p in profiles]
    provenance = gather_provenance(specs_profiles)
    # provenance.authoritative records the TRUTH (tree==origin, clean, tool
    # unmodified). `--allow-nonauthoritative` does NOT change that truth — it
    # lets the cells classify on their REAL numbers (not FAIL_STALE) for local
    # debugging, while the board still records authoritative=false and the gate
    # is told not to auto-fail on staleness.
    authoritative = bool(provenance.get("authoritative", True))
    effective_authoritative = authoritative or ns.allow_nonauthoritative
    if not authoritative:
        print(
            "[scoreboard] *** WARNING: local tree diverges from origin; benchmark "
            "is non-authoritative unless explicitly requested ***",
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

    # --- PyPy / Codon comparator lanes (council Lane C) --------------------
    pypy_bin = _resolve_pypy(ns.pypy) if ns.pypy is not None else None
    pypy_version = _probe_interp_version(pypy_bin) if pypy_bin else None
    if pypy_bin:
        print(
            f"[scoreboard] PyPy comparator: {pypy_bin} ({pypy_version})",
            file=sys.stderr,
        )
    codon_bin = _resolve_codon(ns.codon) if ns.codon is not None else None
    codon_runner = CodonRunner(codon_bin) if codon_bin else None
    codon_version = _probe_codon_version(codon_bin) if codon_bin else None
    if codon_bin:
        print(
            f"[scoreboard] Codon comparator: {codon_bin} ({codon_version})",
            file=sys.stderr,
        )

    budgets = _load_cold_start_budgets()

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
            cell_budget_ms = _budget_ms_for(budgets, backend_name, profile)
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
                        budget_ms=cell_budget_ms,
                        authoritative=effective_authoritative,
                        pypy_bin=pypy_bin,
                        codon_runner=codon_runner,
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
                        provenance=provenance,
                        pypy_version=pypy_version,
                        codon_version=codon_version,
                    )
                    print(
                        f"    -> {cell.verdict}  warm={_fmt(cell.warm_speedup)} "
                        f"cold={_fmt(cell.cold_speedup)} tax={_fmt(cell.startup_tax_ms, 0)}ms",
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

    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=benchmarks_run,
        benchmarks_deferred=benchmarks_deferred,
        cpython_version=cpython_version,
        samples=ns.samples,
        warmup=ns.warmup,
        provenance=provenance,
        pypy_version=pypy_version,
        codon_version=codon_version,
    )
    # Attach the regressions-from-last-green list so print_summary can surface
    # it in the classified output (council ruling A section).
    doc["_out_path"] = str(out_path)
    _attach_regressions(doc)
    doc.pop("_out_path", None)
    out_path.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    if partial_path.exists():
        partial_path.unlink()
    print(f"\nscoreboard JSON -> {out_path}", file=sys.stderr)

    if ns.self_test:
        # The self-test PROVES the pipeline + schema, not the perf/stale gate.
        # It inherently dirties the tree (the tool under test is modified), so
        # subjecting it to FAIL_STALE would be circular — it validates the
        # SCHEMA and returns on that alone.
        problems = _validate_schema(doc)
        print_summary(doc)
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

    print_summary(doc)

    if ns.baseline is not None:
        baseline_path = (
            _latest_baseline(exclude=out_path)
            if ns.baseline == "__latest__"
            else Path(ns.baseline)
        )
        if baseline_path is None or not baseline_path.exists():
            print("[baseline] no prior scoreboard to diff against.", file=sys.stderr)
        else:
            newly_red, regressed = diff_against_baseline(doc, baseline_path)
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

    return _gate_exit_code(
        doc,
        no_gate=ns.no_gate,
        strict_cold=ns.strict_cold,
        allow_nonauthoritative=ns.allow_nonauthoritative,
    )


def _attach_regressions(doc: dict) -> None:
    """Compute REGRESSIONS FROM LAST GREEN vs the latest committed board.

    Surfaced in print_summary's classified output. Best-effort: a missing/older
    baseline simply leaves the section empty. Excludes the board being written
    now (``_out_path``) AND any ``.partial.json`` so it never near-self-diffs.
    """
    out_path = doc.get("_out_path")
    baseline = _latest_baseline(exclude=Path(out_path) if out_path else None)
    if baseline is None or not baseline.exists():
        return
    _newly, regressed = diff_against_baseline(doc, baseline)
    if regressed:
        doc["_regressions_from_last_green"] = regressed


def _checkpoint(
    path: Path,
    cells: list[Cell],
    benchmarks_run: list[str],
    benchmarks_deferred: list[dict],
    cpython_version: str,
    samples: int,
    warmup: int,
    *,
    provenance: dict | None = None,
    pypy_version: str | None = None,
    codon_version: str | None = None,
) -> None:
    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=benchmarks_run,
        benchmarks_deferred=benchmarks_deferred,
        cpython_version=cpython_version,
        samples=samples,
        warmup=warmup,
        provenance=provenance,
        pypy_version=pypy_version,
        codon_version=codon_version,
    )
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)


def _resolve_pypy(arg: str) -> str | None:
    """Resolve a PyPy interpreter path (explicit, or auto-detect 3.11/3.10)."""
    import shutil

    if arg and arg != "__auto__":
        return arg if Path(arg).exists() else shutil.which(arg)
    for cand in (
        "/opt/homebrew/bin/pypy3.11",
        "/opt/homebrew/bin/pypy3.10",
        "/opt/homebrew/bin/pypy3",
        shutil.which("pypy3.11") or "",
        shutil.which("pypy3.10") or "",
        shutil.which("pypy3") or "",
    ):
        if cand and Path(cand).exists():
            return cand
    return None


def _resolve_codon(arg: str) -> str | None:
    """Resolve a Codon binary path (explicit, or auto-detect ~/.codon/bin)."""
    import shutil

    if arg and arg != "__auto__":
        return arg if Path(arg).exists() else shutil.which(arg)
    default = Path.home() / ".codon" / "bin" / "codon"
    if default.exists():
        return str(default)
    return shutil.which("codon")


def _probe_interp_version(interp_bin: str | None) -> str | None:
    if not interp_bin:
        return None
    try:
        res = subprocess.run(
            [interp_bin, "--version"], capture_output=True, text=True, timeout=30
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    out = (res.stdout or res.stderr or "").strip().splitlines()
    return out[0].replace("Python ", "") if out else None


def _probe_codon_version(codon_bin: str | None) -> str | None:
    if not codon_bin:
        return None
    try:
        res = subprocess.run(
            [codon_bin, "--version"], capture_output=True, text=True, timeout=30
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    out = (res.stdout or res.stderr or "").strip()
    return f"codon {out}" if out else None


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
