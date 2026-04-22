from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from enum import Enum
from pathlib import Path
from typing import Any


class LayerStatus(Enum):
    PASS = "pass"
    FAIL = "fail"
    SKIP = "skip"


@dataclass
class LayerResult:
    name: str
    status: LayerStatus
    duration_s: float
    details: str | None = None
    metrics: dict[str, Any] = field(default_factory=dict)

    @property
    def passed(self) -> bool:
        return self.status is LayerStatus.PASS


@dataclass
class HarnessReport:
    profile: str
    results: list[LayerResult]
    timestamp: str | None = None

    def __post_init__(self) -> None:
        if self.timestamp is None:
            self.timestamp = datetime.now(timezone.utc).isoformat()

    @property
    def all_passed(self) -> bool:
        return all(r.passed for r in self.results)

    @property
    def total_duration_s(self) -> float:
        return sum(r.duration_s for r in self.results)

    @property
    def pass_count(self) -> int:
        return sum(1 for r in self.results if r.passed)

    @property
    def fail_count(self) -> int:
        return sum(1 for r in self.results if not r.passed)

    def to_json(self) -> str:
        data = {
            "profile": self.profile,
            "timestamp": self.timestamp,
            "all_passed": self.all_passed,
            "total_duration_s": self.total_duration_s,
            "pass_count": self.pass_count,
            "fail_count": self.fail_count,
            "results": [
                {
                    "name": r.name,
                    "status": r.status.value,
                    "duration_s": r.duration_s,
                    "details": r.details,
                    "metrics": r.metrics,
                }
                for r in self.results
            ],
        }
        return json.dumps(data, indent=2)

    def to_console_table(self) -> str:
        lines: list[str] = []
        for r in self.results:
            status_label = r.status.name
            duration_str = f"{r.duration_s:.2f}s"
            detail_str = f"  {r.details}" if r.details else ""
            lines.append(
                f"  {r.name:<16} {status_label:<6} {duration_str:>6}{detail_str}"
            )
        return "\n".join(lines)

    def save(self, reports_dir: str | Path) -> Path:
        reports_dir = Path(reports_dir)
        reports_dir.mkdir(parents=True, exist_ok=True)
        ts = (
            self.timestamp.replace(":", "-").replace("+", "_")
            if self.timestamp
            else "unknown"
        )
        filename = f"harness-{self.profile}-{ts}.json"
        path = reports_dir / filename
        path.write_text(self.to_json(), encoding="utf-8")
        return path


@dataclass
class Baseline:
    """Stored quality metrics for ratchet enforcement.

    Test counts must never decrease. Performance metrics use lower-is-better
    semantics (nanoseconds, bytes) — the ratchet keeps the lowest achieved value.
    """

    test_counts: dict[str, int] = field(default_factory=dict)
    metrics: dict[str, float] = field(default_factory=dict)

    @classmethod
    def empty(cls) -> Baseline:
        return cls()

    @classmethod
    def from_report(cls, report: HarnessReport) -> Baseline:
        test_counts: dict[str, int] = {}
        metrics: dict[str, float] = {}
        for r in report.results:
            # Accept either "test_count" or "tests_passed" as the canonical
            # test-count metric for baseline ratcheting.
            tc_key = "test_count" if "test_count" in r.metrics else "tests_passed"
            if tc_key in r.metrics:
                test_counts[r.name] = int(r.metrics[tc_key])
            for k, v in r.metrics.items():
                if k not in ("test_count", "tests_passed") and isinstance(
                    v, (int, float)
                ):
                    metrics[k] = float(v)
        return cls(test_counts=test_counts, metrics=metrics)

    def ratchet(self, report: HarnessReport) -> Baseline:
        """Return a new baseline with floors raised where the report improved."""
        new = Baseline.from_report(report)
        merged_counts = dict(self.test_counts)
        for k, v in new.test_counts.items():
            merged_counts[k] = max(merged_counts.get(k, 0), v)
        merged_metrics = dict(self.metrics)
        for k, v in new.metrics.items():
            merged_metrics[k] = min(merged_metrics.get(k, float("inf")), v)
        return Baseline(test_counts=merged_counts, metrics=merged_metrics)

    def check(self, report: HarnessReport) -> list[str]:
        """Return list of violation messages if the report regresses."""
        current = Baseline.from_report(report)
        violations: list[str] = []
        for k, floor in self.test_counts.items():
            actual = current.test_counts.get(k, 0)
            if actual < floor:
                violations.append(
                    f"test count regression: {k} has {actual} tests, baseline requires {floor}"
                )
        return violations

    def save(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            json.dumps(
                {
                    "test_counts": self.test_counts,
                    "metrics": self.metrics,
                },
                indent=2,
            )
        )

    @classmethod
    def load(cls, path: Path) -> Baseline:
        if not path.exists():
            return cls.empty()
        data = json.loads(path.read_text())
        return cls(
            test_counts=data.get("test_counts", {}),
            metrics={k: float(v) for k, v in data.get("metrics", {}).items()},
        )
