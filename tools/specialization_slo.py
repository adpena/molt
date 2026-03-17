#!/usr/bin/env python3
"""Type-hint specialization telemetry with SLO guardrails (MOL-216).

Tracks the effectiveness of type-hint specialization across the Molt
compilation pipeline and enforces service-level objectives:

  SLO guardrails:
    - Compile-time regression: auto-disable if specialization causes >10%
      compile-time increase vs. the unspecialized baseline.
    - Code-size regression: auto-disable if specialization causes >20%
      code-size increase vs. the unspecialized baseline.
    - Success rate floor: warn if specialization success rate drops below 80%.

Metrics are exposed as a dictionary suitable for integration with the
dashboard delta protocol (MOL-214).

Usage:
    # As a library:
    from tools.specialization_slo import SpecializationSLO, SpecializationSample
    slo = SpecializationSLO.from_env()
    slo.record(SpecializationSample(...))
    report = slo.evaluate()

    # CLI:
    uv run --python 3.12 python3 tools/specialization_slo.py \\
        --current bench/results/bench.json \\
        --baseline bench/baseline.json

    uv run --python 3.12 python3 tools/specialization_slo.py --status
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import threading
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]

# ---------------------------------------------------------------------------
# SLO thresholds (configurable via env vars)
# ---------------------------------------------------------------------------

DEFAULT_COMPILE_TIME_REGRESSION_LIMIT = 0.10  # 10%
DEFAULT_CODE_SIZE_REGRESSION_LIMIT = 0.20  # 20%
DEFAULT_SUCCESS_RATE_FLOOR = 0.80  # 80%
DEFAULT_MIN_SAMPLES = 5  # need at least N samples before enforcing


# ---------------------------------------------------------------------------
# Data model
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class SpecializationSample:
    """A single specialization attempt measurement."""

    function_name: str
    # Did specialization succeed (type info sufficient)?
    succeeded: bool
    # Compile time with specialization enabled (seconds).
    compile_time_specialized_s: float
    # Compile time without specialization (seconds).
    compile_time_baseline_s: float
    # Code size with specialization (bytes).
    code_size_specialized_bytes: int
    # Code size without specialization (bytes).
    code_size_baseline_bytes: int
    # Timestamp of the measurement.
    timestamp: float = 0.0

    @property
    def compile_time_change_pct(self) -> float:
        if self.compile_time_baseline_s <= 0:
            return 0.0
        return (
            (self.compile_time_specialized_s - self.compile_time_baseline_s)
            / self.compile_time_baseline_s
        )

    @property
    def code_size_change_pct(self) -> float:
        if self.code_size_baseline_bytes <= 0:
            return 0.0
        return (
            (self.code_size_specialized_bytes - self.code_size_baseline_bytes)
            / self.code_size_baseline_bytes
        )


@dataclass
class SLOViolation:
    """A single SLO violation."""

    metric: str
    threshold: float
    actual: float
    severity: str  # "warn", "error"
    message: str
    auto_disabled: bool = False


@dataclass
class SLOReport:
    """Evaluation result for all specialization SLOs."""

    timestamp: float
    total_samples: int
    success_count: int
    failure_count: int
    success_rate: float
    avg_compile_time_change_pct: float
    avg_code_size_change_pct: float
    p95_compile_time_change_pct: float
    p95_code_size_change_pct: float
    violations: list[SLOViolation]
    specialization_enabled: bool
    auto_disabled_reason: str | None = None

    @property
    def has_violations(self) -> bool:
        return len(self.violations) > 0

    def to_dict(self) -> dict[str, Any]:
        d = asdict(self)
        return d

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), indent=2)

    def dashboard_metrics(self) -> dict[str, Any]:
        """Return a flat dict suitable for dashboard delta integration."""
        return {
            "spec_total_samples": self.total_samples,
            "spec_success_rate": round(self.success_rate, 4),
            "spec_avg_compile_time_change_pct": round(self.avg_compile_time_change_pct, 4),
            "spec_avg_code_size_change_pct": round(self.avg_code_size_change_pct, 4),
            "spec_p95_compile_time_change_pct": round(self.p95_compile_time_change_pct, 4),
            "spec_p95_code_size_change_pct": round(self.p95_code_size_change_pct, 4),
            "spec_enabled": self.specialization_enabled,
            "spec_violation_count": len(self.violations),
            "spec_auto_disabled_reason": self.auto_disabled_reason,
        }


# ---------------------------------------------------------------------------
# Core SLO evaluator
# ---------------------------------------------------------------------------

class SpecializationSLO:
    """Thread-safe SLO evaluator for type-hint specialization.

    Tracks samples, evaluates guardrails, and can auto-disable
    specialization when thresholds are breached.
    """

    def __init__(
        self,
        *,
        compile_time_limit: float = DEFAULT_COMPILE_TIME_REGRESSION_LIMIT,
        code_size_limit: float = DEFAULT_CODE_SIZE_REGRESSION_LIMIT,
        success_rate_floor: float = DEFAULT_SUCCESS_RATE_FLOOR,
        min_samples: int = DEFAULT_MIN_SAMPLES,
        max_history: int = 10_000,
    ) -> None:
        self._lock = threading.Lock()
        self._compile_time_limit = compile_time_limit
        self._code_size_limit = code_size_limit
        self._success_rate_floor = success_rate_floor
        self._min_samples = min_samples
        self._max_history = max_history

        self._samples: list[SpecializationSample] = []
        self._enabled: bool = True
        self._auto_disabled_reason: str | None = None
        self._total_recorded: int = 0

    @classmethod
    def from_env(cls) -> SpecializationSLO:
        """Create an SLO evaluator with thresholds from environment variables."""

        def _env_float(key: str, default: float) -> float:
            raw = os.environ.get(key, "").strip()
            if not raw:
                return default
            try:
                return float(raw)
            except ValueError:
                return default

        def _env_int(key: str, default: int) -> int:
            raw = os.environ.get(key, "").strip()
            if not raw:
                return default
            try:
                return int(raw)
            except ValueError:
                return default

        return cls(
            compile_time_limit=_env_float(
                "MOLT_SPEC_SLO_COMPILE_TIME_LIMIT",
                DEFAULT_COMPILE_TIME_REGRESSION_LIMIT,
            ),
            code_size_limit=_env_float(
                "MOLT_SPEC_SLO_CODE_SIZE_LIMIT",
                DEFAULT_CODE_SIZE_REGRESSION_LIMIT,
            ),
            success_rate_floor=_env_float(
                "MOLT_SPEC_SLO_SUCCESS_RATE_FLOOR",
                DEFAULT_SUCCESS_RATE_FLOOR,
            ),
            min_samples=_env_int("MOLT_SPEC_SLO_MIN_SAMPLES", DEFAULT_MIN_SAMPLES),
        )

    # -- Recording ----------------------------------------------------------

    def record(self, sample: SpecializationSample) -> None:
        """Record a specialization attempt measurement."""
        with self._lock:
            self._samples.append(sample)
            self._total_recorded += 1
            # Evict old samples.
            overflow = len(self._samples) - self._max_history
            if overflow > 0:
                del self._samples[:overflow]

    def record_batch(self, samples: list[SpecializationSample]) -> None:
        """Record multiple samples atomically."""
        with self._lock:
            self._samples.extend(samples)
            self._total_recorded += len(samples)
            overflow = len(self._samples) - self._max_history
            if overflow > 0:
                del self._samples[:overflow]

    # -- Evaluation ---------------------------------------------------------

    def evaluate(self) -> SLOReport:
        """Evaluate all SLOs against current samples.

        If any error-level SLO is breached and there are enough samples,
        specialization is auto-disabled.
        """
        with self._lock:
            return self._evaluate_locked()

    def _evaluate_locked(self) -> SLOReport:
        now = time.time()
        n = len(self._samples)
        violations: list[SLOViolation] = []

        if n == 0:
            return SLOReport(
                timestamp=now,
                total_samples=0,
                success_count=0,
                failure_count=0,
                success_rate=1.0,
                avg_compile_time_change_pct=0.0,
                avg_code_size_change_pct=0.0,
                p95_compile_time_change_pct=0.0,
                p95_code_size_change_pct=0.0,
                violations=[],
                specialization_enabled=self._enabled,
                auto_disabled_reason=self._auto_disabled_reason,
            )

        successes = [s for s in self._samples if s.succeeded]
        failures = [s for s in self._samples if not s.succeeded]
        success_rate = len(successes) / n if n > 0 else 1.0

        ct_changes = [s.compile_time_change_pct for s in self._samples]
        cs_changes = [s.code_size_change_pct for s in self._samples]

        avg_ct = sum(ct_changes) / len(ct_changes) if ct_changes else 0.0
        avg_cs = sum(cs_changes) / len(cs_changes) if cs_changes else 0.0

        p95_ct = _percentile(ct_changes, 0.95)
        p95_cs = _percentile(cs_changes, 0.95)

        # --- Check SLOs ---

        # 1. Success rate floor (warning only).
        if n >= self._min_samples and success_rate < self._success_rate_floor:
            violations.append(SLOViolation(
                metric="success_rate",
                threshold=self._success_rate_floor,
                actual=success_rate,
                severity="warn",
                message=(
                    f"Specialization success rate {success_rate:.1%} "
                    f"below floor {self._success_rate_floor:.0%}"
                ),
            ))

        # 2. Compile-time regression guardrail (error: auto-disable).
        if n >= self._min_samples and avg_ct > self._compile_time_limit:
            v = SLOViolation(
                metric="compile_time_regression",
                threshold=self._compile_time_limit,
                actual=avg_ct,
                severity="error",
                message=(
                    f"Avg compile-time regression {avg_ct:.1%} "
                    f"exceeds limit {self._compile_time_limit:.0%}"
                ),
                auto_disabled=True,
            )
            violations.append(v)
            if self._enabled:
                self._enabled = False
                self._auto_disabled_reason = v.message
                _log_guardrail(v)

        # 3. Code-size regression guardrail (error: auto-disable).
        if n >= self._min_samples and avg_cs > self._code_size_limit:
            v = SLOViolation(
                metric="code_size_regression",
                threshold=self._code_size_limit,
                actual=avg_cs,
                severity="error",
                message=(
                    f"Avg code-size regression {avg_cs:.1%} "
                    f"exceeds limit {self._code_size_limit:.0%}"
                ),
                auto_disabled=True,
            )
            violations.append(v)
            if self._enabled:
                self._enabled = False
                self._auto_disabled_reason = v.message
                _log_guardrail(v)

        return SLOReport(
            timestamp=now,
            total_samples=n,
            success_count=len(successes),
            failure_count=len(failures),
            success_rate=success_rate,
            avg_compile_time_change_pct=avg_ct,
            avg_code_size_change_pct=avg_cs,
            p95_compile_time_change_pct=p95_ct,
            p95_code_size_change_pct=p95_cs,
            violations=violations,
            specialization_enabled=self._enabled,
            auto_disabled_reason=self._auto_disabled_reason,
        )

    # -- Manual control -----------------------------------------------------

    def force_enable(self) -> None:
        """Manually re-enable specialization after auto-disable."""
        with self._lock:
            self._enabled = True
            self._auto_disabled_reason = None

    def force_disable(self, reason: str = "manual") -> None:
        """Manually disable specialization."""
        with self._lock:
            self._enabled = False
            self._auto_disabled_reason = reason

    @property
    def enabled(self) -> bool:
        with self._lock:
            return self._enabled


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _percentile(data: list[float], pct: float) -> float:
    if not data:
        return 0.0
    s = sorted(data)
    k = (len(s) - 1) * pct
    f = int(k)
    c = f + 1
    if c >= len(s):
        return s[-1]
    return s[f] + (k - f) * (s[c] - s[f])


def _log_guardrail(v: SLOViolation) -> None:
    print(
        f"molt specialization guardrail: AUTO-DISABLE - {v.message}",
        file=sys.stderr,
        flush=True,
    )


# ---------------------------------------------------------------------------
# CLI: evaluate from benchmark JSON files
# ---------------------------------------------------------------------------

def _load_bench_metrics(path: Path) -> list[dict[str, Any]]:
    """Load benchmark entries from a Molt bench JSON file."""
    text = path.read_text(encoding="utf-8")
    data = json.loads(text)
    if isinstance(data, list):
        return data
    if isinstance(data, dict):
        # Support {"benchmarks": [...]} wrapper.
        if "benchmarks" in data:
            return data["benchmarks"]
        return [data]
    return []


def _samples_from_bench_pair(
    current: list[dict[str, Any]],
    baseline: list[dict[str, Any]],
) -> list[SpecializationSample]:
    """Build SpecializationSample list from paired benchmark data.

    Pairs benchmarks by name.  The "current" run is treated as the
    specialized variant and "baseline" as unspecialized.
    """
    baseline_map = {b.get("name", ""): b for b in baseline if "name" in b}
    samples: list[SpecializationSample] = []
    for entry in current:
        name = entry.get("name", "")
        base = baseline_map.get(name)
        if base is None:
            continue
        ct_spec = entry.get("molt_build_s", entry.get("compile_time_s", 0.0))
        ct_base = base.get("molt_build_s", base.get("compile_time_s", 0.0))
        cs_spec = int(entry.get("molt_size_kb", entry.get("code_size_kb", 0)) * 1024)
        cs_base = int(base.get("molt_size_kb", base.get("code_size_kb", 0)) * 1024)
        succeeded = entry.get("specialization_applied", True)

        samples.append(SpecializationSample(
            function_name=name,
            succeeded=bool(succeeded),
            compile_time_specialized_s=float(ct_spec),
            compile_time_baseline_s=float(ct_base),
            code_size_specialized_bytes=cs_spec,
            code_size_baseline_bytes=cs_base,
            timestamp=time.time(),
        ))
    return samples


def _print_report(report: SLOReport, *, as_json: bool = False) -> None:
    if as_json:
        print(report.to_json())
        return

    is_tty = sys.stdout.isatty()

    def _c(code: str, text: str) -> str:
        return f"\033[{code}m{text}\033[0m" if is_tty else text

    print(f"\n{'=' * 60}")
    print("  Type-Hint Specialization SLO Report (MOL-216)")
    print(f"{'=' * 60}")
    print(f"  Samples:       {report.total_samples}")
    print(f"  Success rate:  {report.success_rate:.1%} "
          f"({report.success_count}/{report.total_samples})")
    print(f"  Compile-time:  avg {report.avg_compile_time_change_pct:+.2%} "
          f" p95 {report.p95_compile_time_change_pct:+.2%}")
    print(f"  Code-size:     avg {report.avg_code_size_change_pct:+.2%} "
          f" p95 {report.p95_code_size_change_pct:+.2%}")
    status = _c("32", "ENABLED") if report.specialization_enabled else _c("31", "DISABLED")
    print(f"  Specialization: {status}")
    if report.auto_disabled_reason:
        print(f"  Reason:        {report.auto_disabled_reason}")

    if report.violations:
        print(f"\n  {'Violations':}")
        for v in report.violations:
            sev = _c("31", "ERROR") if v.severity == "error" else _c("33", "WARN")
            print(f"    [{sev}] {v.message}")
    else:
        print(f"\n  {_c('32', 'All SLOs met.')}")

    print(f"{'=' * 60}\n")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Type-hint specialization SLO guardrails (MOL-216)",
    )
    parser.add_argument(
        "--current",
        type=Path,
        help="Path to current benchmark JSON (specialized build).",
    )
    parser.add_argument(
        "--baseline",
        type=Path,
        help="Path to baseline benchmark JSON (unspecialized build).",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Output report as JSON.",
    )
    parser.add_argument(
        "--status",
        action="store_true",
        help="Print current SLO status (no benchmark data needed).",
    )
    parser.add_argument(
        "--dashboard-metrics",
        action="store_true",
        help="Output metrics dict for dashboard delta integration.",
    )
    args = parser.parse_args()

    slo = SpecializationSLO.from_env()

    if args.current and args.baseline:
        current_data = _load_bench_metrics(args.current)
        baseline_data = _load_bench_metrics(args.baseline)
        samples = _samples_from_bench_pair(current_data, baseline_data)
        if not samples:
            print("No matching benchmarks found between current and baseline.",
                  file=sys.stderr)
            sys.exit(2)
        slo.record_batch(samples)

    report = slo.evaluate()

    if args.dashboard_metrics:
        print(json.dumps(report.dashboard_metrics(), indent=2))
        return

    _print_report(report, as_json=args.json)

    if report.has_violations and any(
        v.severity == "error" for v in report.violations
    ):
        sys.exit(1)


if __name__ == "__main__":
    main()
