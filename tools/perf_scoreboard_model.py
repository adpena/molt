#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import statistics
import sys
from dataclasses import dataclass, field
from pathlib import Path

import harness_memory_guard
import perf_causality
from perf_schema import (
    CLASS_DIMENSIONAL_WIN,
    CLASS_GREEN,
    CLASS_INFRA,
    CLASS_RED_NOISY,
    CLASS_RED_STABLE,
    CLASS_TIE,
    CLASSIFY_STATES as CLASSIFY_STATES,
    RED_THRESHOLD,
    UNSTABLE_CV,
    VERDICT_BUILD_FAILED,
    VERDICT_CPY_INCOMPAT,
    VERDICT_FAIL_COLD_BUDGET,
    VERDICT_FAIL_ENGINE,
    VERDICT_FAIL_STALE,
    VERDICT_GREEN,
    VERDICT_RUN_BLOCKED,
    VERDICT_RUN_ERROR,
    VERDICT_UNSTABLE,
    VERDICT_WARN_COLD_FLOOR,
)


REPO_ROOT = Path(__file__).resolve().parents[1]

TOOLS_ROOT = REPO_ROOT / "tools"

SRC_ROOT = REPO_ROOT / "src"

if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.llvm_toolchain import (  # noqa: E402
    LlvmToolchainConfigError,
    required_llvm_backend_pin,
)

SAFE_RUN = TOOLS_ROOT / "safe_run.py"

SCOREBOARD_DIR = REPO_ROOT / "bench" / "scoreboard"

COLD_START_BUDGET_PATH = SCOREBOARD_DIR / "cold_start_budget.json"

PERFSCORE_SESSION_ID = "perfscore"

DIMENSIONAL_WIN_MIN_FRACTION = 0.05

NON_AUTHORITATIVE_NOTE = (
    "non-authoritative board (origin/main mismatch, dirty tree, "
    "tool-modified, or quiescence authority failed)"
)

_VERDICT_DERIVED_NOTES = frozenset(
    {
        NON_AUTHORITATIVE_NOTE,
        # Legacy notes may exist in stored boards; keep rebuild/merge summary
        # able to clear and rederive them.
        "non-authoritative tree (local != origin/main or dirty)",
    }
)


class ScoreboardSchemaError(RuntimeError):
    """Raised when a CPython scoreboard document violates schema authority."""

    def __init__(self, context: str, problems: list[str]) -> None:
        self.context = context
        self.problems = problems
        detail = "; ".join(problems)
        super().__init__(f"{context}: {detail}")


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


DEFAULT_RUN_RSS_MB = 4096

DEFAULT_RUN_TIMEOUT_S = 120.0

SAFE_RUN_POLL_S = 0.01

DEFAULT_SAMPLES = 5

DEFAULT_WARMUP = 2

RUN_BLOCKED_BACKENDS = {"wasm"}


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

PROFILE_BUILD_FLAG = {
    "release-fast": "release",
    "release-output": "release",  # same CLI flag; distinguished by env below
    "dev-fast": "dev",
}


def _llvm_backend_pin():
    try:
        return required_llvm_backend_pin(REPO_ROOT)
    except LlvmToolchainConfigError:
        return None


def _llvm_sys_prefix_env_var() -> str | None:
    pin = _llvm_backend_pin()
    return None if pin is None else pin.env_var


def _llvm_sys_prefix() -> str | None:
    """Resolve the LLVM_SYS prefix the manifest-pinned LLVM backend needs."""
    pin = _llvm_backend_pin()
    if pin is None:
        return None
    explicit = os.environ.get(pin.env_var, "").strip()
    if explicit:
        return explicit
    candidate = Path(f"/opt/homebrew/opt/llvm@{pin.major}")
    if candidate.exists():
        return str(candidate)
    return None


@dataclass
class RunOutcome:
    ok: bool
    elapsed_s: float | None
    peak_rss_mib: float | None
    status: str  # "ok" | "timeout" | "oom" | "error" | "nonzero"
    exit_code: int | None
    stdout: str | None = None
    stdout_tail: str | None = None
    stderr_tail: str | None = None


def _tail_text(text: str | None, *, max_chars: int = 4096) -> str | None:
    if not text:
        return None
    if len(text) <= max_chars:
        return text
    return text[-max_chars:]


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
    proc = harness_memory_guard.guarded_completed_process(
        full,
        prefix="MOLT_BENCH",
        env=env,
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=timeout_s + 30.0,
    )
    if proc.timed_out:
        return RunOutcome(
            False,
            None,
            None,
            "timeout",
            None,
            stdout_tail=_tail_text(proc.stdout),
            stderr_tail=_tail_text(proc.stderr),
        )

    payload = _parse_safe_run_line(proc.stderr or "")
    if payload is None:
        return RunOutcome(
            False,
            None,
            None,
            "error",
            proc.returncode,
            stdout_tail=_tail_text(proc.stdout),
            stderr_tail=_tail_text(proc.stderr),
        )
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
        stdout_tail=_tail_text(proc.stdout),
        stderr_tail=_tail_text(proc.stderr),
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
    floor; a single CPython GC/scheduler spike (or one anomalously-fast
    cold-cache sample) must not throw out a cell where molt wins decisively and
    is itself stable. The cell is stable iff:
      * molt is stable (low CV), AND
      * EITHER CPython is also stable,
        OR the warm verdict is robust to CPython's spread WITH the single most
        extreme sample on each side TRIMMED — i.e. the warm_speedup
        (cpython/molt, on the molt median) keeps the same side of the 1.00 floor
        whether computed with CPython's 2nd-fastest or 2nd-slowest sample. The
        trim makes the bound robust to exactly one outlier (the dominant failure
        mode: one GC spike OR one fast cold-cache run) rather than using the raw
        min/max, which a lone outlier drags to the flip point. This is a
        median-of-bounds robustness test per pyperf discipline, never a per-test
        special case.
    """
    if not molt.stable:
        return False
    if cpy.stable:
        return True
    if molt.median_s is None or molt.median_s <= 0:
        return False
    samples = sorted(cpy.samples_s or [])
    if len(samples) < 3:
        # Too few to trim an outlier robustly; fall back to raw min/max.
        lo_s, hi_s = (samples[0], samples[-1]) if samples else (None, None)
    else:
        # Trim the single most-extreme sample on each side.
        lo_s, hi_s = samples[1], samples[-2]
    if lo_s is None or hi_s is None:
        return False
    lo = lo_s / molt.median_s
    hi = hi_s / molt.median_s
    both_win = lo > RED_THRESHOLD and hi > RED_THRESHOLD
    both_lose = lo <= RED_THRESHOLD and hi <= RED_THRESHOLD
    return both_win or both_lose


def _warm_speedup_ci(
    speedups: list[float],
) -> tuple[float | None, float | None, float | None, float | None]:
    """(median, variance, ci_lo, ci_hi) of per-pass warm_speedups.

    The CI is a Student-t 95% interval on the mean of the passes when n>=2 (the
    small-sample correct choice; z would understate the interval at n=5). With a
    single pass there is no spread to bound — ci is (None, None) and the caller
    must treat the pass as not-yet-confirmed (we never fabricate a tight CI from
    one sample). Variance is the sample variance (n-1).
    """
    vals = [s for s in speedups if isinstance(s, (int, float))]
    if not vals:
        return None, None, None, None
    median = statistics.median(vals)
    if len(vals) < 2:
        return round(median, 4), None, None, None
    mean = statistics.mean(vals)
    var = statistics.variance(vals)
    stdev = var**0.5
    sem = stdev / (len(vals) ** 0.5)
    # 95% two-sided Student-t critical values for small n (df = n-1). Table is
    # exact for the n we use (2..10); beyond that we clamp to the n=10 value
    # (1.833) which is conservative-enough and avoids a scipy dependency.
    t_table = {
        1: 12.706,
        2: 4.303,
        3: 3.182,
        4: 2.776,
        5: 2.571,
        6: 2.447,
        7: 2.365,
        8: 2.306,
        9: 2.262,
    }
    df = len(vals) - 1
    tcrit = t_table.get(df, 2.262 if df >= 9 else 12.706)
    half = tcrit * sem
    return round(median, 4), round(var, 6), round(mean - half, 4), round(mean + half, 4)


def _repeat_stability(ci_lo: float | None, ci_hi: float | None) -> str:
    """Classify the repeat CI vs the 1.00 floor.

    Returns one of: 'STABLE_BELOW' (CI entirely < 1.0 — a real red),
    'STABLE_ABOVE' (CI entirely > 1.0 — a real green), 'STRADDLES' (CI crosses
    1.0 — a TIE), or 'UNCONFIRMED' (a single pass; no CI to judge).
    """
    if ci_lo is None or ci_hi is None:
        return "UNCONFIRMED"
    if ci_hi < RED_THRESHOLD:
        return "STABLE_BELOW"
    if ci_lo > RED_THRESHOLD:
        return "STABLE_ABOVE"
    return "STRADDLES"


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

    # WARM: steady-state median for each runtime.
    warm_molt_s: float | None = None
    warm_cpython_s: float | None = None

    # Peak RSS (the worse of cold/warm samples) for each runtime.
    molt_peak_rss_mib: float | None = None
    cpython_peak_rss_mib: float | None = None

    # Stability + diagnostic note.
    stable: bool = False
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
    fact_class: str | None = None
    pypy_advantage_class: str | None = None
    reference_class: str | None = None
    codon_semantics: str | None = None
    attribution_confidence: float | None = None

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

    # --- Measurement-hygiene classification (#69 --classify) --------------
    # The council's 5-state classification (orthogonal to ``verdict``): the
    # answer to "TRUE compiler target or measurement artifact?". Set only when
    # --classify is requested; None otherwise so the 2-D verdict path is
    # unchanged for callers that do not opt in.
    classification: str | None = None  # one of CLASSIFY_STATES
    classification_reason: str | None = None
    measured_quiescent: bool | None = None

    # --- Repeat-pass CI (#69 --repeat N) ----------------------------------
    # Per-pass warm_speedups + the Student-t 95% CI; a verdict is STABLE only if
    # the CI does not straddle 1.00. Empty/None when --repeat is not used.
    repeat_passes: int | None = None
    repeat_warm_speedups: list[float] | None = None
    repeat_median_warm: float | None = None
    repeat_variance: float | None = None
    repeat_ci_lo: float | None = None
    repeat_ci_hi: float | None = None
    repeat_stability: str | None = (
        None  # STABLE_BELOW|STABLE_ABOVE|STRADDLES|UNCONFIRMED
    )

    # --- Cycle attribution (#69 Rule 1 + --emit-cycle-profile) ------------
    # The CYCLE profile (NOT alloc-count) for warm reds — the next-opt signal.
    cycle_profile: dict | None = None

    def finalize(
        self,
        *,
        budget_ms: float | None = None,
        authoritative: bool = True,
    ) -> None:
        """Derive speedups + the TWO-DIMENSIONAL verdict from collected facts.

        ``budget_ms`` is the cold-start tax budget for this (backend, profile)
        cell (None = no budget recorded yet; FAIL_COLD_BUDGET cannot fire).
        ``authoritative`` False stamps every cell FAIL_STALE (the board cannot
        be the canonical origin/main performance contract) — overriding all
        other verdicts per council ruling A.

        Sets ``verdict`` (the VERDICT_* vocabulary), the single gate authority
        consumed by summaries, rebuilds, merges, and CI.
        """
        self.cold_budget_ms = budget_ms

        # FAIL_STALE overrides everything: a non-authoritative board's numbers
        # are not the origin/main contract, full stop.
        if not authoritative:
            self.verdict = VERDICT_FAIL_STALE
            if self.note is None:
                self.note = NON_AUTHORITATIVE_NOTE
            return

        if self.run_blocked:
            self.verdict = VERDICT_RUN_BLOCKED
            return
        if not self.build_ok:
            self.verdict = VERDICT_BUILD_FAILED
            return
        # CPython baseline can't run -> no valid floor; not gated.
        if not self.cpython_ok:
            self.cpython_incompatible = True
            self.verdict = VERDICT_CPY_INCOMPAT
            if self.note is None:
                self.note = "CPython baseline could not run this script standalone"
            return
        # CPython runs but molt does not -> a real molt run failure.
        if not self.molt_ok:
            self.verdict = VERDICT_RUN_ERROR
            if self.note is None:
                self.note = "molt run failed/unmeasurable while CPython ran"
            return

        self.warm_speedup = _safe_ratio(self.warm_cpython_s, self.warm_molt_s)
        self.cold_speedup = _safe_ratio(self.cold_cpython_s, self.cold_molt_s)

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
            _apply_perf_attribution(self)
            return
        # 2. FAIL_COLD_BUDGET — warm is fine but the fixed startup tax exceeds
        #    the recorded budget for this lane. A startup regression, not an
        #    engine red; routes to the cold-start lane.
        if over_budget:
            self.verdict = VERDICT_FAIL_COLD_BUDGET
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
            self.suspected_startup_component = (
                self.suspected_startup_component
                or _suspect_startup_component(self.benchmark)
            )
            return
        # 4. GREEN — warm fast, cold fast, within budget.
        self.verdict = VERDICT_GREEN


@dataclass(frozen=True)
class CpythonOracle:
    """Host-native CPython interpreter chosen for the perf floor."""

    cmd: tuple[str, ...]
    executable: str
    version: str
    implementation: str
    sys_platform: str
    machine: str
    arch: str
    pointer_bits: int

    @property
    def display(self) -> str:
        return " ".join(self.cmd)

    def host_metadata(self) -> dict:
        return {
            "cmd": list(self.cmd),
            "executable": self.executable,
            "implementation": self.implementation,
            "version": self.version,
            "sys_platform": self.sys_platform,
            "machine": self.machine,
            "arch": self.arch,
            "pointer_bits": self.pointer_bits,
        }


def _safe_ratio(numerator: float | None, denominator: float | None) -> float | None:
    if numerator is None or denominator is None or denominator <= 0:
        return None
    return round(numerator / denominator, 4)


def classify_cell(
    cell: "Cell",
    *,
    quiescent: bool,
    baseline_cell: dict | None = None,
) -> tuple[str, str]:
    """Return (classification, reason) — the 5-state measurement-hygiene verdict.

    ``quiescent`` is the machine-quiescence result for the WHOLE board (a single
    run cannot be partly quiet). ``baseline_cell`` (optional) enables
    DIMENSIONAL_WIN detection: when the warm gate did not flip but a non-warm
    dimension (alloc/RSS/binary-size/cold/compile) improved materially vs the
    baseline. Pure function of the cell's finalized facts + these two inputs;
    unit-tested with synthetic cells (no molt build).
    """
    # Infra states have no warm number to classify — pass them through.
    if cell.verdict in (
        VERDICT_BUILD_FAILED,
        VERDICT_RUN_ERROR,
        VERDICT_RUN_BLOCKED,
        VERDICT_CPY_INCOMPAT,
        VERDICT_FAIL_STALE,
    ):
        return CLASS_INFRA, f"infrastructure verdict {cell.verdict} (no warm number)"

    warm = cell.warm_speedup
    if warm is None:
        return CLASS_INFRA, "no warm_speedup measured"

    # Determine the CI posture if a repeat sweep ran; else fall back to the
    # single-pass robust-stability flag (cell.stable).
    rep = cell.repeat_stability
    has_ci = rep in ("STABLE_BELOW", "STABLE_ABOVE", "STRADDLES")

    # --- TIE: the CI genuinely crosses 1.00 (neither win nor loss) ----------
    # A straddling CI is the council's TIE — even on a quiet machine, the data do
    # not place the cell on one side of the floor. A warm==1.00 single-pass cell
    # (the borderline FAIL_ENGINE ties) is ALSO a TIE under classification: it is
    # not a representational loss, it is statistically CPython. BUT (Rule 4) a
    # warm-tie cell that improved a NON-warm dimension materially vs a baseline is
    # a DIMENSIONAL_WIN, not a bare TIE — the "landed without a warm flip but
    # better elsewhere" lane. So consult the dimensional check before TIE.
    if has_ci and rep == "STRADDLES":
        dim = _dimensional_improvement(cell, baseline_cell)
        if dim is not None:
            return CLASS_DIMENSIONAL_WIN, dim
        return (
            CLASS_TIE,
            f"repeat CI [{cell.repeat_ci_lo}, {cell.repeat_ci_hi}] straddles 1.00 "
            f"(n={cell.repeat_passes}) — neither a win nor a loss",
        )
    if not has_ci and abs(warm - RED_THRESHOLD) < 1e-9:
        dim = _dimensional_improvement(cell, baseline_cell)
        if dim is not None:
            return CLASS_DIMENSIONAL_WIN, dim
        return (
            CLASS_TIE,
            "warm_speedup == 1.00 — statistically CPython (a tie, not a loss); "
            "run --repeat to bound the CI",
        )

    # When a repeat CI exists it is AUTHORITATIVE over the lone point estimate —
    # that is the council's entire reason for --repeat (a flaky point sample must
    # not decide the side of the floor). A STABLE_BELOW CI is a red even if one
    # pass read > 1.0; a STABLE_ABOVE CI is a green even if one pass read < 1.0.
    # Only without a CI does the point estimate decide. (STRADDLES was already
    # routed to TIE above, so here has_ci implies STABLE_BELOW xor STABLE_ABOVE.)
    if has_ci:
        warm_below = rep == "STABLE_BELOW"
        warm_above = rep == "STABLE_ABOVE"
    else:
        warm_below = warm < RED_THRESHOLD
        warm_above = warm > RED_THRESHOLD

    # --- WARM RED branch (CI-governed when present) -------------------------
    if warm_below:
        # RED_STABLE iff: quiescent AND robustly stable AND a repeat CI EXISTS
        # and sits entirely below 1.0. A single-pass red (no CI) is NOT yet
        # RED_STABLE — it is RED_NOISY until --repeat confirms it (a point
        # estimate is not a confidence interval).
        if quiescent and cell.stable and has_ci:
            return (
                CLASS_RED_STABLE,
                f"repeat CI [{cell.repeat_ci_lo}, {cell.repeat_ci_hi}] clears below "
                f"1.00 (point {warm}x), quiescent + stable — TRUE compiler target",
            )
        # Demote to RED_NOISY and NAME why it is not yet a target.
        causes = []
        if not quiescent:
            causes.append("machine NOT quiescent (contaminated)")
        if not cell.stable:
            causes.append("cell unstable (CV/robustness)")
        if not has_ci:
            causes.append("no repeat CI (single pass — run --repeat N to confirm)")
        return (
            CLASS_RED_NOISY,
            f"warm point {warm}x below 1.00 BUT " + "; ".join(causes),
        )

    # --- WARM GREEN / DIMENSIONAL branch (CI-governed when present) ----------
    if warm_above:
        # ASYMMETRY OF CONTAMINATION: a competing build steals cycles from the
        # TIMED molt process, so it can only make molt look SLOWER — never
        # artificially FASTER. Therefore a warm-ABOVE cell under contamination is
        # a CONSERVATIVE green (the quiet number would be >= this); the win is
        # real. Only cell INSTABILITY (CV/robustness) can undermine a green —
        # NOT non-quiescence. (Contrast the warm-BELOW branch, where load CAN
        # manufacture a false red, so quiescence is mandatory there.) Calling a
        # 10x cell RED_NOISY merely because an idle daemon was running is wrong.
        if cell.stable:
            note = f"warm point {warm}x above 1.00, stable"
            if has_ci:
                note += (
                    f", repeat CI [{cell.repeat_ci_lo}, {cell.repeat_ci_hi}] clears "
                    "above 1.00"
                )
            if not quiescent:
                note += (
                    " (measured non-quiescent — contamination is conservative for "
                    "a green: the quiet number can only be faster; board authority "
                    "still gated)"
                )
            else:
                note += " + quiescent"
            return CLASS_GREEN, note
        # warm>1 but the cell is UNSTABLE -> not a confirmed win (a straddle was
        # already caught above; this is point-above with a volatile sample).
        return (
            CLASS_RED_NOISY,
            f"warm point {warm}x above 1.00 BUT cell unstable (CV/robustness) — "
            "green unconfirmed",
        )

    # --- DIMENSIONAL_WIN: warm gate flat, another dimension improved --------
    # Reached only when warm is neither clearly below nor clearly above (e.g. a
    # tie that a baseline shows improved on alloc/RSS/size/cold). Needs a
    # baseline; without one we cannot assert a dimensional improvement.
    dim = _dimensional_improvement(cell, baseline_cell)
    if dim is not None:
        return CLASS_DIMENSIONAL_WIN, dim
    return (
        CLASS_TIE,
        "warm at the 1.00 floor and no material dimensional improvement vs baseline",
    )


def _dimensional_improvement(cell: "Cell", baseline_cell: dict | None) -> str | None:
    """Detect a material non-warm improvement vs a baseline cell (DIMENSIONAL_WIN).

    Compares RSS, binary size, cold_speedup, and compile time against the
    baseline. Returns a human reason naming the improved dimension(s) if ANY beat
    the baseline by >= DIMENSIONAL_WIN_MIN_FRACTION; else None. A warm flip is
    NOT a dimensional win (that is GREEN); this is the "landed without a warm
    flip but materially better elsewhere" lane (Rule 4).
    """
    if not baseline_cell:
        return None
    wins: list[str] = []

    def _improved_lower(new: float | None, old: float | None, label: str) -> None:
        # Lower-is-better dimensions (RSS, size, compile time).
        if new is None or old is None or old <= 0:
            return
        frac = (old - new) / old
        if frac >= DIMENSIONAL_WIN_MIN_FRACTION:
            wins.append(f"{label} {old:.1f}->{new:.1f} (-{frac * 100:.0f}%)")

    def _improved_higher(new: float | None, old: float | None, label: str) -> None:
        # Higher-is-better dimensions (cold_speedup).
        if new is None or old is None or old <= 0:
            return
        frac = (new - old) / old
        if frac >= DIMENSIONAL_WIN_MIN_FRACTION:
            wins.append(f"{label} {old:.2f}->{new:.2f} (+{frac * 100:.0f}%)")

    _improved_lower(
        cell.molt_peak_rss_mib, baseline_cell.get("molt_peak_rss_mib"), "RSS"
    )
    _improved_lower(
        cell.binary_size_kib, baseline_cell.get("binary_size_kib"), "binary"
    )
    _improved_lower(cell.compile_time_s, baseline_cell.get("compile_time_s"), "compile")
    _improved_higher(cell.cold_speedup, baseline_cell.get("cold_speedup"), "cold")
    if not wins:
        return None
    return "warm gate flat, but DIMENSIONAL win: " + "; ".join(wins)


def _apply_perf_attribution(cell: Cell) -> None:
    attribution = perf_causality.derive_cell_attribution(
        {"benchmark": cell.benchmark, "cycle_profile": cell.cycle_profile}
    )
    cell.fact_class = attribution.fact_class
    cell.suspected_missing_fact = attribution.suspected_missing_fact
    cell.pypy_advantage_class = attribution.pypy_advantage_class
    cell.reference_class = attribution.reference_class
    cell.codon_semantics = attribution.codon_semantics
    cell.attribution_confidence = attribution.attribution_confidence


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
