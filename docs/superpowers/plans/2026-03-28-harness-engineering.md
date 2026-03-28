# Harness Engineering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `molt harness` — a unified, local-first quality enforcement command with layered profiles (quick/standard/deep) and zero-tolerance hard gates.

**Architecture:** Python orchestrator (`src/molt/harness.py`) dispatches to cargo, pytest, and Node.js. A Rust library crate (`runtime/molt-harness/`) handles resource enforcement verification, audit validation, benchmarks, and size tracking. A test corpus (`tests/harness/`) holds Python test files, Jinja2 templates, baselines, and reports.

**Tech Stack:** Python 3.11+ (orchestrator), Rust (test library, criterion benchmarks), cargo-nextest, cargo-mutants, Jinja2 (test generation), trybuild (compile-fail), libfuzzer-sys (fuzz)

---

## File Map

### New Files

| File | Responsibility |
|------|---------------|
| `src/molt/harness.py` | Orchestrator: profile selection, layer sequencing, subprocess dispatch |
| `src/molt/harness_layers.py` | Individual layer implementations (compile, lint, unit, wasm, etc.) |
| `src/molt/harness_report.py` | Result types, console table, JSON export, HTML report generation |
| `runtime/molt-harness/Cargo.toml` | Rust test library crate config |
| `runtime/molt-harness/src/lib.rs` | Public API for Rust-side test infrastructure |
| `runtime/molt-harness/src/resource_enforcement.rs` | WASM resource limit scenario runner |
| `runtime/molt-harness/src/audit_verification.rs` | Audit event schema validator |
| `runtime/molt-harness/src/size_tracking.rs` | Binary/WASM output size measurement |
| `tests/harness/baselines/baseline.json` | Stored metrics (test counts, sizes, perf) |
| `tests/harness/corpus/resource/time_limit.py` | Resource enforcement scenario: infinite loop |
| `tests/harness/corpus/resource/memory_limit.py` | Resource enforcement scenario: alloc loop |
| `tests/harness/corpus/resource/dos_pow.py` | Resource enforcement scenario: 2**10_000_000 |
| `tests/harness/corpus/resource/dos_repeat.py` | Resource enforcement scenario: 'x'*10B |
| `tests/harness/corpus/resource/alloc_limit.py` | Resource enforcement scenario: rapid alloc |
| `tests/harness/corpus/resource/recursion_limit.py` | Resource enforcement scenario: deep recursion |
| `tests/harness/templates/type_x_operator.py.j2` | Jinja2 template: types crossed with operators |

### Modified Files

| File | Change |
|------|--------|
| `src/molt/cli.py` | Register `harness` subcommand with profile args |
| `Cargo.toml` (workspace) | Add `runtime/molt-harness` to members |

---

### Task 1: Core Result Types and Report Module

**Files:**
- Create: `src/molt/harness_report.py`

- [ ] **Step 1: Write the test for LayerResult and HarnessReport**

Create `tests/test_harness_report.py`:

```python
"""Tests for harness result types and reporting."""
import json
import sys
sys.path.insert(0, "src")

from molt.harness_report import LayerResult, HarnessReport, LayerStatus


def test_layer_result_pass():
    r = LayerResult(name="compile", status=LayerStatus.PASS, duration_s=1.2)
    assert r.passed
    assert r.duration_s == 1.2


def test_layer_result_fail_with_details():
    r = LayerResult(
        name="unit-rust",
        status=LayerStatus.FAIL,
        duration_s=4.3,
        details="2 failures: test_a, test_b",
    )
    assert not r.passed
    assert "test_a" in r.details


def test_layer_result_skip():
    r = LayerResult(name="fuzz", status=LayerStatus.SKIP, duration_s=0.0)
    assert not r.passed
    assert r.status == LayerStatus.SKIP


def test_harness_report_all_pass():
    report = HarnessReport(profile="quick", results=[
        LayerResult(name="compile", status=LayerStatus.PASS, duration_s=1.0),
        LayerResult(name="lint", status=LayerStatus.PASS, duration_s=0.5),
    ])
    assert report.all_passed
    assert report.total_duration_s == 1.5
    assert report.pass_count == 2
    assert report.fail_count == 0


def test_harness_report_with_failure():
    report = HarnessReport(profile="standard", results=[
        LayerResult(name="compile", status=LayerStatus.PASS, duration_s=1.0),
        LayerResult(name="lint", status=LayerStatus.FAIL, duration_s=0.5, details="1 warning"),
    ])
    assert not report.all_passed
    assert report.fail_count == 1


def test_harness_report_to_json():
    report = HarnessReport(profile="quick", results=[
        LayerResult(name="compile", status=LayerStatus.PASS, duration_s=1.0),
    ])
    j = report.to_json()
    parsed = json.loads(j)
    assert parsed["profile"] == "quick"
    assert parsed["all_passed"] is True
    assert len(parsed["results"]) == 1
    assert parsed["results"][0]["name"] == "compile"
    assert parsed["results"][0]["status"] == "pass"


def test_harness_report_console_table():
    report = HarnessReport(profile="quick", results=[
        LayerResult(name="compile", status=LayerStatus.PASS, duration_s=1.2),
        LayerResult(name="lint", status=LayerStatus.FAIL, duration_s=0.8, details="1 warning"),
    ])
    table = report.to_console_table()
    assert "compile" in table
    assert "PASS" in table
    assert "FAIL" in table
    assert "1.2s" in table


def test_harness_report_metrics():
    report = HarnessReport(profile="deep", results=[
        LayerResult(
            name="bench",
            status=LayerStatus.PASS,
            duration_s=60.0,
            metrics={"fib_30_ns": 12345, "binary_size_bytes": 4096},
        ),
    ])
    j = json.loads(report.to_json())
    assert j["results"][0]["metrics"]["fib_30_ns"] == 12345
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PYTHONPATH=src python3 -m pytest tests/test_harness_report.py -v`
Expected: `ModuleNotFoundError: No module named 'molt.harness_report'`

- [ ] **Step 3: Implement harness_report.py**

Create `src/molt/harness_report.py`:

```python
"""Harness result types and report generation.

Every layer produces a LayerResult. The orchestrator collects them into a
HarnessReport which can be printed as a console table, saved as JSON,
or rendered as HTML.
"""
from __future__ import annotations

import json
import time
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any, Optional


class LayerStatus(Enum):
    PASS = "pass"
    FAIL = "fail"
    SKIP = "skip"


@dataclass
class LayerResult:
    """Result of a single harness layer execution."""
    name: str
    status: LayerStatus
    duration_s: float
    details: str = ""
    metrics: dict[str, Any] = field(default_factory=dict)

    @property
    def passed(self) -> bool:
        return self.status == LayerStatus.PASS


@dataclass
class HarnessReport:
    """Collected results from a full harness run."""
    profile: str
    results: list[LayerResult]
    timestamp: float = field(default_factory=time.time)

    @property
    def all_passed(self) -> bool:
        return all(r.passed for r in self.results)

    @property
    def total_duration_s(self) -> float:
        return sum(r.duration_s for r in self.results)

    @property
    def pass_count(self) -> int:
        return sum(1 for r in self.results if r.status == LayerStatus.PASS)

    @property
    def fail_count(self) -> int:
        return sum(1 for r in self.results if r.status == LayerStatus.FAIL)

    def to_json(self) -> str:
        """Serialize to JSON for storage and trend tracking."""
        return json.dumps({
            "profile": self.profile,
            "timestamp": self.timestamp,
            "all_passed": self.all_passed,
            "total_duration_s": round(self.total_duration_s, 2),
            "pass_count": self.pass_count,
            "fail_count": self.fail_count,
            "results": [
                {
                    "name": r.name,
                    "status": r.status.value,
                    "duration_s": round(r.duration_s, 2),
                    "details": r.details,
                    "metrics": r.metrics,
                }
                for r in self.results
            ],
        }, indent=2)

    def to_console_table(self) -> str:
        """Format as a human-readable console table."""
        lines = [f"\nmolt harness {self.profile}\n"]
        for r in self.results:
            status = "PASS" if r.passed else ("SKIP" if r.status == LayerStatus.SKIP else "FAIL")
            dur = f"{r.duration_s:.1f}s"
            detail = f"   {r.details}" if r.details else ""
            lines.append(f"  {r.name:<16} {status:<6} {dur:>6}{detail}")
        passed = self.pass_count
        total = len(self.results)
        dur = f"{self.total_duration_s:.1f}s"
        lines.append(f"\n  RESULT: {passed}/{total} layers passed in {dur}\n")
        return "\n".join(lines)

    def save(self, reports_dir: Path) -> Path:
        """Save JSON report to the reports directory."""
        reports_dir.mkdir(parents=True, exist_ok=True)
        ts = time.strftime("%Y-%m-%d-%H%M%S")
        path = reports_dir / f"{ts}.json"
        path.write_text(self.to_json())
        return path
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `PYTHONPATH=src python3 -m pytest tests/test_harness_report.py -v`
Expected: All 8 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/molt/harness_report.py tests/test_harness_report.py
git commit -m "feat(harness): add LayerResult and HarnessReport types with JSON/console output"
```

---

### Task 2: Baseline and Ratchet System

**Files:**
- Create: `tests/harness/baselines/baseline.json`
- Modify: `src/molt/harness_report.py` (add baseline comparison)

- [ ] **Step 1: Write the test for baseline loading, comparison, and ratchet**

Append to `tests/test_harness_report.py`:

```python
from molt.harness_report import Baseline


def test_baseline_empty():
    b = Baseline.empty()
    assert b.test_counts == {}
    assert b.metrics == {}


def test_baseline_from_report():
    report = HarnessReport(profile="deep", results=[
        LayerResult(name="unit-rust", status=LayerStatus.PASS, duration_s=4.0,
                    metrics={"test_count": 40}),
        LayerResult(name="bench", status=LayerStatus.PASS, duration_s=60.0,
                    metrics={"fib_30_ns": 12345, "binary_size_bytes": 4096}),
    ])
    b = Baseline.from_report(report)
    assert b.test_counts["unit-rust"] == 40
    assert b.metrics["fib_30_ns"] == 12345


def test_baseline_ratchet_raises_floor():
    old = Baseline(test_counts={"unit-rust": 30}, metrics={"fib_30_ns": 15000})
    new_report = HarnessReport(profile="deep", results=[
        LayerResult(name="unit-rust", status=LayerStatus.PASS, duration_s=4.0,
                    metrics={"test_count": 40}),
        LayerResult(name="bench", status=LayerStatus.PASS, duration_s=60.0,
                    metrics={"fib_30_ns": 12000}),
    ])
    updated = old.ratchet(new_report)
    assert updated.test_counts["unit-rust"] == 40  # raised from 30
    assert updated.metrics["fib_30_ns"] == 12000   # faster = new floor


def test_baseline_ratchet_never_lowers():
    old = Baseline(test_counts={"unit-rust": 40}, metrics={"fib_30_ns": 10000})
    worse_report = HarnessReport(profile="deep", results=[
        LayerResult(name="unit-rust", status=LayerStatus.PASS, duration_s=4.0,
                    metrics={"test_count": 35}),
        LayerResult(name="bench", status=LayerStatus.PASS, duration_s=60.0,
                    metrics={"fib_30_ns": 15000}),
    ])
    updated = old.ratchet(worse_report)
    assert updated.test_counts["unit-rust"] == 40   # not lowered
    assert updated.metrics["fib_30_ns"] == 10000     # not raised (worse)


def test_baseline_check_violations():
    baseline = Baseline(test_counts={"unit-rust": 40}, metrics={"fib_30_ns": 10000})
    report = HarnessReport(profile="deep", results=[
        LayerResult(name="unit-rust", status=LayerStatus.PASS, duration_s=4.0,
                    metrics={"test_count": 38}),
    ])
    violations = baseline.check(report)
    assert len(violations) == 1
    assert "unit-rust" in violations[0]
    assert "40" in violations[0]


def test_baseline_save_load_roundtrip(tmp_path):
    b = Baseline(test_counts={"unit-rust": 40}, metrics={"fib_30_ns": 12345})
    path = tmp_path / "baseline.json"
    b.save(path)
    loaded = Baseline.load(path)
    assert loaded.test_counts == b.test_counts
    assert loaded.metrics == b.metrics
```

- [ ] **Step 2: Run tests to verify new tests fail**

Run: `PYTHONPATH=src python3 -m pytest tests/test_harness_report.py -v -k baseline`
Expected: `ImportError` — `Baseline` not defined

- [ ] **Step 3: Implement Baseline class**

Add to `src/molt/harness_report.py`:

```python
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
            if "test_count" in r.metrics:
                test_counts[r.name] = int(r.metrics["test_count"])
            for k, v in r.metrics.items():
                if k != "test_count" and isinstance(v, (int, float)):
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
            # Lower is better (ns, bytes) — keep the minimum
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
        path.write_text(json.dumps({
            "test_counts": self.test_counts,
            "metrics": self.metrics,
        }, indent=2))

    @classmethod
    def load(cls, path: Path) -> Baseline:
        if not path.exists():
            return cls.empty()
        data = json.loads(path.read_text())
        return cls(
            test_counts=data.get("test_counts", {}),
            metrics={k: float(v) for k, v in data.get("metrics", {}).items()},
        )
```

- [ ] **Step 4: Run all tests to verify they pass**

Run: `PYTHONPATH=src python3 -m pytest tests/test_harness_report.py -v`
Expected: All 14 tests PASS

- [ ] **Step 5: Create initial baseline file**

Create `tests/harness/baselines/baseline.json`:

```json
{
  "test_counts": {
    "unit-rust": 40,
    "unit-python": 69
  },
  "metrics": {}
}
```

- [ ] **Step 6: Commit**

```bash
git add src/molt/harness_report.py tests/test_harness_report.py tests/harness/baselines/baseline.json
git commit -m "feat(harness): add Baseline ratchet system with save/load/check/ratchet"
```

---

### Task 3: Layer Implementations (quick profile)

**Files:**
- Create: `src/molt/harness_layers.py`

- [ ] **Step 1: Write the test for layer runner functions**

Create `tests/test_harness_layers.py`:

```python
"""Tests for individual harness layer implementations."""
import sys
sys.path.insert(0, "src")

from molt.harness_layers import (
    LAYERS,
    PROFILES,
    get_layers_for_profile,
    run_layer_compile,
    run_layer_lint,
)
from molt.harness_report import LayerStatus


def test_profiles_are_supersets():
    quick = set(get_layers_for_profile("quick"))
    standard = set(get_layers_for_profile("standard"))
    deep = set(get_layers_for_profile("deep"))
    assert quick.issubset(standard)
    assert standard.issubset(deep)


def test_quick_has_four_layers():
    layers = get_layers_for_profile("quick")
    assert [l.name for l in layers] == ["compile", "lint", "unit-rust", "unit-python"]


def test_standard_adds_four_layers():
    layers = get_layers_for_profile("standard")
    names = [l.name for l in layers]
    assert "wasm-compile" in names
    assert "differential" in names
    assert "resource" in names
    assert "audit" in names


def test_deep_adds_remaining_layers():
    layers = get_layers_for_profile("deep")
    names = [l.name for l in layers]
    for expected in ["fuzz", "conformance", "bench", "size", "mutation",
                     "determinism", "miri", "compile-fail"]:
        assert expected in names, f"missing layer: {expected}"


def test_layer_definitions_have_required_fields():
    for layer in LAYERS:
        assert layer.name, "layer must have a name"
        assert layer.profile in ("quick", "standard", "deep"), f"bad profile: {layer.profile}"
        assert callable(layer.run_fn), f"layer {layer.name} must have a callable run_fn"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PYTHONPATH=src python3 -m pytest tests/test_harness_layers.py -v`
Expected: `ModuleNotFoundError`

- [ ] **Step 3: Implement harness_layers.py**

Create `src/molt/harness_layers.py`:

```python
"""Individual harness layer implementations.

Each layer is a function that takes a HarnessConfig and returns a LayerResult.
Layers are registered in the LAYERS list with their profile assignment.
"""
from __future__ import annotations

import os
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Optional

from molt.harness_report import LayerResult, LayerStatus


@dataclass
class HarnessConfig:
    """Configuration for a harness run."""
    project_root: Path
    fail_fast: bool = True
    fuzz_duration_s: int = 600
    molt_cmd: str = "molt"
    verbose: bool = False

    @property
    def molt_crates(self) -> list[str]:
        """Crates authored by the molt team (subject to lint/check gates)."""
        return [
            "molt-runtime", "molt-backend", "molt-snapshot",
            "molt-embed", "molt-harness",
        ]


@dataclass
class LayerDef:
    """Definition of a harness layer."""
    name: str
    profile: str  # "quick", "standard", or "deep"
    run_fn: Callable[[HarnessConfig], LayerResult]


def _run_cmd(
    args: list[str],
    cwd: Optional[Path] = None,
    env: Optional[dict] = None,
    timeout: int = 300,
) -> tuple[int, str, str]:
    """Run a subprocess and return (exit_code, stdout, stderr)."""
    merged_env = {**os.environ, **(env or {})}
    try:
        result = subprocess.run(
            args,
            cwd=cwd,
            env=merged_env,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        return result.returncode, result.stdout, result.stderr
    except subprocess.TimeoutExpired:
        return 1, "", f"timeout after {timeout}s"
    except FileNotFoundError:
        return 1, "", f"command not found: {args[0]}"


# ── Layer: compile ──────────────────────────────────────────────────

def run_layer_compile(config: HarnessConfig) -> LayerResult:
    start = time.monotonic()
    crate_args = []
    for c in config.molt_crates:
        crate_args.extend(["-p", c])
    code, stdout, stderr = _run_cmd(
        ["cargo", "check"] + crate_args,
        cwd=config.project_root,
    )
    duration = time.monotonic() - start
    if code != 0:
        return LayerResult(
            name="compile", status=LayerStatus.FAIL,
            duration_s=duration, details=stderr[-500:] if stderr else "unknown error",
        )
    # Check for warnings in stderr (cargo check outputs warnings to stderr)
    warning_count = stderr.count("warning:")
    # Filter out warnings from dependency crates — only count molt-authored
    molt_warnings = sum(
        1 for line in stderr.splitlines()
        if "warning:" in line and any(c in line for c in config.molt_crates)
    )
    if molt_warnings > 0:
        return LayerResult(
            name="compile", status=LayerStatus.FAIL,
            duration_s=duration,
            details=f"{molt_warnings} warning(s) in molt-authored crates",
        )
    return LayerResult(name="compile", status=LayerStatus.PASS, duration_s=duration)


# ── Layer: lint ─────────────────────────────────────────────────────

def run_layer_lint(config: HarnessConfig) -> LayerResult:
    start = time.monotonic()
    crate_args = []
    for c in config.molt_crates:
        crate_args.extend(["-p", c])
    code, stdout, stderr = _run_cmd(
        ["cargo", "clippy"] + crate_args + ["--", "-D", "warnings"],
        cwd=config.project_root,
    )
    duration = time.monotonic() - start
    if code != 0:
        return LayerResult(
            name="lint", status=LayerStatus.FAIL,
            duration_s=duration, details=stderr[-500:] if stderr else "clippy failed",
        )
    return LayerResult(name="lint", status=LayerStatus.PASS, duration_s=duration)


# ── Layer: unit-rust ────────────────────────────────────────────────

def run_layer_unit_rust(config: HarnessConfig) -> LayerResult:
    start = time.monotonic()
    total_tests = 0
    failures: list[str] = []

    # Three feature-flag modes (Monty pattern)
    modes = [
        ("default", []),
        ("refcount_verify", ["--features", "refcount_verify"]),
        ("audit", ["--features", "audit"]),
    ]

    for mode_name, extra_args in modes:
        code, stdout, stderr = _run_cmd(
            ["cargo", "test", "-p", "molt-runtime", "-p", "molt-snapshot",
             "-p", "molt-embed", "--lib"] + extra_args,
            cwd=config.project_root,
            timeout=120,
        )
        # Parse test count from output
        for line in (stdout + stderr).splitlines():
            if "test result:" in line and "passed" in line:
                # "test result: ok. 40 passed; 0 failed; ..."
                parts = line.split()
                for i, p in enumerate(parts):
                    if p == "passed;":
                        total_tests += int(parts[i - 1])
                    if p == "failed;":
                        fail_n = int(parts[i - 1])
                        if fail_n > 0:
                            failures.append(f"{mode_name}: {fail_n} failures")

    duration = time.monotonic() - start
    if failures:
        return LayerResult(
            name="unit-rust", status=LayerStatus.FAIL,
            duration_s=duration, details="; ".join(failures),
        )
    return LayerResult(
        name="unit-rust", status=LayerStatus.PASS,
        duration_s=duration,
        details=f"{total_tests} tests across 3 modes",
        metrics={"test_count": total_tests},
    )


# ── Layer: unit-python ──────────────────────────────────────────────

def run_layer_unit_python(config: HarnessConfig) -> LayerResult:
    start = time.monotonic()
    failures: list[str] = []
    test_count = 0

    # Run capability manifest self-tests
    code, stdout, stderr = _run_cmd(
        ["python3", "-m", "molt.capability_manifest"],
        cwd=config.project_root,
        env={"PYTHONPATH": str(config.project_root / "src")},
    )
    if code != 0:
        failures.append(f"capability_manifest: {stderr[-200:]}")
    else:
        for line in stdout.splitlines():
            if "tests passed" in line.lower():
                # "69/69 tests passed."
                parts = line.split("/")
                if parts:
                    try:
                        test_count += int(parts[0].strip())
                    except ValueError:
                        pass

    # Verify REPL module imports
    code, stdout, stderr = _run_cmd(
        ["python3", "-c", "from molt.repl import run_repl; print('OK')"],
        cwd=config.project_root,
        env={"PYTHONPATH": str(config.project_root / "src")},
    )
    if code != 0 or "OK" not in stdout:
        failures.append("repl import failed")

    duration = time.monotonic() - start
    if failures:
        return LayerResult(
            name="unit-python", status=LayerStatus.FAIL,
            duration_s=duration, details="; ".join(failures),
        )
    return LayerResult(
        name="unit-python", status=LayerStatus.PASS,
        duration_s=duration,
        details=f"{test_count} tests",
        metrics={"test_count": test_count},
    )


# ── Layer stubs for standard/deep (implemented in later tasks) ──────

def _stub_layer(name: str) -> Callable[[HarnessConfig], LayerResult]:
    def run(config: HarnessConfig) -> LayerResult:
        return LayerResult(name=name, status=LayerStatus.SKIP, duration_s=0.0,
                           details="not yet implemented")
    return run


# ── Layer registry ──────────────────────────────────────────────────

LAYERS: list[LayerDef] = [
    LayerDef("compile", "quick", run_layer_compile),
    LayerDef("lint", "quick", run_layer_lint),
    LayerDef("unit-rust", "quick", run_layer_unit_rust),
    LayerDef("unit-python", "quick", run_layer_unit_python),
    LayerDef("wasm-compile", "standard", _stub_layer("wasm-compile")),
    LayerDef("differential", "standard", _stub_layer("differential")),
    LayerDef("resource", "standard", _stub_layer("resource")),
    LayerDef("audit", "standard", _stub_layer("audit")),
    LayerDef("fuzz", "deep", _stub_layer("fuzz")),
    LayerDef("conformance", "deep", _stub_layer("conformance")),
    LayerDef("bench", "deep", _stub_layer("bench")),
    LayerDef("size", "deep", _stub_layer("size")),
    LayerDef("mutation", "deep", _stub_layer("mutation")),
    LayerDef("determinism", "deep", _stub_layer("determinism")),
    LayerDef("miri", "deep", _stub_layer("miri")),
    LayerDef("compile-fail", "deep", _stub_layer("compile-fail")),
]

PROFILES = {
    "quick": ["compile", "lint", "unit-rust", "unit-python"],
    "standard": ["compile", "lint", "unit-rust", "unit-python",
                  "wasm-compile", "differential", "resource", "audit"],
    "deep": [l.name for l in LAYERS],
}


def get_layers_for_profile(profile: str) -> list[LayerDef]:
    """Return the ordered list of layers for a profile."""
    names = PROFILES.get(profile, PROFILES["standard"])
    by_name = {l.name: l for l in LAYERS}
    return [by_name[n] for n in names if n in by_name]
```

- [ ] **Step 4: Run all tests to verify they pass**

Run: `PYTHONPATH=src python3 -m pytest tests/test_harness_layers.py -v`
Expected: All 5 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/molt/harness_layers.py tests/test_harness_layers.py
git commit -m "feat(harness): add layer definitions, quick profile implementations, registry"
```

---

### Task 4: Orchestrator and CLI Integration

**Files:**
- Create: `src/molt/harness.py`
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Write the test for the orchestrator**

Create `tests/test_harness_orchestrator.py`:

```python
"""Tests for the harness orchestrator."""
import sys
sys.path.insert(0, "src")

from molt.harness import run_harness
from molt.harness_report import LayerStatus


def test_run_harness_quick_returns_report(tmp_path):
    """Quick profile should return a report with 4 layers."""
    # This is an integration test — it actually runs cargo check etc.
    # Skip if cargo is not available.
    import shutil
    if not shutil.which("cargo"):
        import pytest
        pytest.skip("cargo not available")

    from molt.harness_layers import HarnessConfig
    from pathlib import Path

    # Use the actual project root
    project_root = Path(__file__).parent.parent
    config = HarnessConfig(project_root=project_root)
    report = run_harness("quick", config)
    assert len(report.results) == 4
    assert report.results[0].name == "compile"


def test_run_harness_fail_fast_stops_on_failure():
    """When fail_fast=True, layers after a failure are skipped."""
    from molt.harness import _run_profile
    from molt.harness_layers import LayerDef, HarnessConfig
    from pathlib import Path

    call_log = []

    def pass_layer(config):
        call_log.append("pass")
        return LayerResult(name="pass", status=LayerStatus.PASS, duration_s=0.1)

    def fail_layer(config):
        call_log.append("fail")
        return LayerResult(name="fail", status=LayerStatus.FAIL, duration_s=0.1)

    def skip_layer(config):
        call_log.append("should-not-run")
        return LayerResult(name="skip", status=LayerStatus.PASS, duration_s=0.1)

    from molt.harness_report import LayerResult

    layers = [
        LayerDef("pass", "quick", pass_layer),
        LayerDef("fail", "quick", fail_layer),
        LayerDef("skip", "quick", skip_layer),
    ]
    config = HarnessConfig(project_root=Path("."), fail_fast=True)
    report = _run_profile(layers, config)
    assert call_log == ["pass", "fail"]
    assert len(report.results) == 3
    assert report.results[2].status == LayerStatus.SKIP
```

- [ ] **Step 2: Run test to verify it fails**

Run: `PYTHONPATH=src python3 -m pytest tests/test_harness_orchestrator.py -v -k fail_fast`
Expected: `ModuleNotFoundError`

- [ ] **Step 3: Implement harness.py**

Create `src/molt/harness.py`:

```python
"""Molt harness orchestrator.

Dispatches layers in profile order, collects results, manages baselines,
and produces reports. This is the entry point called by `molt harness`.
"""
from __future__ import annotations

import sys
import time
from pathlib import Path
from typing import Optional

from molt.harness_layers import HarnessConfig, LayerDef, get_layers_for_profile
from molt.harness_report import (
    Baseline,
    HarnessReport,
    LayerResult,
    LayerStatus,
)

REPORTS_DIR = Path("tests/harness/reports")
BASELINE_PATH = Path("tests/harness/baselines/baseline.json")


def _run_profile(
    layers: list[LayerDef],
    config: HarnessConfig,
) -> HarnessReport:
    """Execute layers in order, respecting fail_fast."""
    results: list[LayerResult] = []
    failed = False

    for layer in layers:
        if failed and config.fail_fast:
            results.append(LayerResult(
                name=layer.name,
                status=LayerStatus.SKIP,
                duration_s=0.0,
                details="skipped due to prior failure",
            ))
            continue

        result = layer.run_fn(config)
        results.append(result)

        if not result.passed and result.status != LayerStatus.SKIP:
            failed = True

    return HarnessReport(profile=config.project_root.name, results=results)


def run_harness(
    profile: str,
    config: HarnessConfig,
    check_baseline: bool = True,
) -> HarnessReport:
    """Run the harness with the given profile.

    Returns a HarnessReport. Prints console table to stderr.
    Saves JSON report to tests/harness/reports/.
    Checks baseline if available and check_baseline is True.
    """
    layers = get_layers_for_profile(profile)
    report = _run_profile(layers, config)
    report.profile = profile

    # Print console table
    print(report.to_console_table(), file=sys.stderr)

    # Save report
    report_path = report.save(config.project_root / REPORTS_DIR)

    # Check baseline
    if check_baseline:
        baseline_path = config.project_root / BASELINE_PATH
        baseline = Baseline.load(baseline_path)
        violations = baseline.check(report)
        if violations:
            print("\nBASELINE VIOLATIONS:", file=sys.stderr)
            for v in violations:
                print(f"  - {v}", file=sys.stderr)

    return report


def main(args: Optional[list[str]] = None) -> int:
    """CLI entry point for `molt harness`."""
    import argparse

    parser = argparse.ArgumentParser(description="Molt quality harness")
    parser.add_argument("profile", nargs="?", default="standard",
                        choices=["quick", "standard", "deep"],
                        help="Test profile (default: standard)")
    parser.add_argument("--no-fail-fast", action="store_true",
                        help="Continue running layers after failure")
    parser.add_argument("--verbose", "-v", action="store_true")
    parser.add_argument("--json", action="store_true",
                        help="Print JSON report to stdout")

    parsed = parser.parse_args(args)

    project_root = Path.cwd()
    config = HarnessConfig(
        project_root=project_root,
        fail_fast=not parsed.no_fail_fast,
        verbose=parsed.verbose,
    )

    report = run_harness(parsed.profile, config)

    if parsed.json:
        print(report.to_json())

    return 0 if report.all_passed else 1
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `PYTHONPATH=src python3 -m pytest tests/test_harness_orchestrator.py -v -k fail_fast`
Expected: PASS

- [ ] **Step 5: Register the harness subcommand in cli.py**

Find the options dict in `src/molt/cli.py` (around line 26375) and add `"harness"`:

```python
"harness": [
    "--no-fail-fast",
    "--json",
    "--verbose",
],
```

Find where subcommand handlers are dispatched (near the `repl` handler added earlier) and add:

```python
if args.command == "harness":
    from molt.harness import main as harness_main
    profile = getattr(args, "profile", "standard")
    harness_args = [profile]
    if getattr(args, "no_fail_fast", False):
        harness_args.append("--no-fail-fast")
    if getattr(args, "verbose", False):
        harness_args.append("--verbose")
    if getattr(args, "json_output", False):
        harness_args.append("--json")
    return harness_main(harness_args)
```

- [ ] **Step 6: Verify CLI integration**

Run: `PYTHONPATH=src python3 -c "from molt.harness import main; print('OK')"`
Expected: `OK`

- [ ] **Step 7: Commit**

```bash
git add src/molt/harness.py tests/test_harness_orchestrator.py src/molt/cli.py
git commit -m "feat(harness): add orchestrator, CLI integration, fail-fast, baseline check"
```

---

### Task 5: Resource Enforcement Test Corpus

**Files:**
- Create: `tests/harness/corpus/resource/time_limit.py`
- Create: `tests/harness/corpus/resource/memory_limit.py`
- Create: `tests/harness/corpus/resource/dos_pow.py`
- Create: `tests/harness/corpus/resource/dos_repeat.py`
- Create: `tests/harness/corpus/resource/alloc_limit.py`
- Create: `tests/harness/corpus/resource/recursion_limit.py`

- [ ] **Step 1: Create the 6 resource enforcement scenario files**

`tests/harness/corpus/resource/time_limit.py`:
```python
# Resource enforcement scenario: infinite loop must be killed by time limit.
# Expected: process terminates within 2s when max_duration=1s.
while True:
    pass
```

`tests/harness/corpus/resource/memory_limit.py`:
```python
# Resource enforcement scenario: allocation loop must be stopped by memory limit.
# Expected: MemoryError (uncatchable) when max_memory=1MB.
data = []
while True:
    data.append(b"x" * 1024)
```

`tests/harness/corpus/resource/dos_pow.py`:
```python
# Resource enforcement scenario: huge exponentiation must be rejected.
# Expected: MemoryError from pre-emptive DoS guard.
result = 2 ** 10_000_000
```

`tests/harness/corpus/resource/dos_repeat.py`:
```python
# Resource enforcement scenario: huge string repetition must be rejected.
# Expected: MemoryError from pre-emptive DoS guard.
result = "x" * 10_000_000_000
```

`tests/harness/corpus/resource/alloc_limit.py`:
```python
# Resource enforcement scenario: rapid allocation must be stopped.
# Expected: allocation limit exceeded when max_allocations=1000.
objects = []
for i in range(10_000):
    objects.append([i])
```

`tests/harness/corpus/resource/recursion_limit.py`:
```python
# Resource enforcement scenario: deep recursion must raise RecursionError.
# Expected: RecursionError when max_recursion_depth=50.
# Note: RecursionError IS catchable (CPython compat).
def recurse(n):
    return recurse(n + 1)

try:
    recurse(0)
except RecursionError:
    print("RecursionError caught correctly")
```

- [ ] **Step 2: Commit**

```bash
git add tests/harness/corpus/resource/
git commit -m "feat(harness): add 6 resource enforcement scenario files"
```

---

### Task 6: Combinatorial Test Templates

**Files:**
- Create: `tests/harness/templates/type_x_operator.py.j2`

- [ ] **Step 1: Create the Jinja2 template**

`tests/harness/templates/type_x_operator.py.j2`:
```jinja2
{# Combinatorial test: cross Python types with binary operators.
   Generated by: molt harness generate-tests
   DO NOT EDIT — regenerate from template. #}
{% set types = {
    "int": ["0", "1", "-1", "42", "2**30"],
    "float": ["0.0", "1.0", "-1.5", "3.14"],
    "bool": ["True", "False"],
} %}
{% set operators = [
    ("+", "add"), ("-", "sub"), ("*", "mul"), ("**", "pow"),
    ("//", "floordiv"), ("%", "mod"),
    ("<<", "lshift"), (">>", "rshift"),
    ("&", "bitand"), ("|", "bitor"), ("^", "bitxor"),
] %}
{% for type_name, values in types.items() %}
{% for op_sym, op_name in operators %}
{% for val in values %}
# {{ type_name }}_{{ op_name }}_{{ loop.index }}
try:
    _result = {{ val }} {{ op_sym }} {{ values[0] }}
    print(f"{{ type_name }}_{{ op_name }}_{{ loop.index }}: {repr(_result)}")
except Exception as e:
    print(f"{{ type_name }}_{{ op_name }}_{{ loop.index }}: {type(e).__name__}: {e}")
{% endfor %}
{% endfor %}
{% endfor %}
```

- [ ] **Step 2: Commit**

```bash
git add tests/harness/templates/
git commit -m "feat(harness): add Jinja2 combinatorial test template for type x operator"
```

---

### Task 7: Rust Harness Crate Scaffold

**Files:**
- Create: `runtime/molt-harness/Cargo.toml`
- Create: `runtime/molt-harness/src/lib.rs`
- Create: `runtime/molt-harness/src/size_tracking.rs`
- Modify: `Cargo.toml` (workspace)

- [ ] **Step 1: Create Cargo.toml**

`runtime/molt-harness/Cargo.toml`:
```toml
[package]
name = "molt-harness"
version = "0.1.0"
edition = "2024"
license = "Apache-2.0"
description = "Quality enforcement test library for Molt"

[dependencies]
sha2 = "0.10"

[dev-dependencies]
```

- [ ] **Step 2: Create lib.rs**

`runtime/molt-harness/src/lib.rs`:
```rust
//! Molt quality enforcement test library.
//!
//! Provides Rust-side infrastructure for the `molt harness` command:
//! - Resource enforcement scenario verification
//! - Audit event schema validation
//! - Binary/WASM size measurement
//! - Determinism checks (native vs WASM output comparison)

pub mod size_tracking;
```

- [ ] **Step 3: Create size_tracking.rs**

`runtime/molt-harness/src/size_tracking.rs`:
```rust
//! Binary and WASM output size measurement.
//!
//! Tracks artifact sizes for regression detection. The Python orchestrator
//! calls this via `cargo test -p molt-harness` to collect size metrics.

use std::path::Path;

/// Measure the size of a file in bytes. Returns 0 if the file does not exist.
pub fn file_size_bytes(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Measure sizes of all artifacts in a directory matching a glob pattern.
pub fn artifact_sizes(dir: &Path, extension: &str) -> Vec<(String, u64)> {
    let mut sizes = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == extension) {
                let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                let size = file_size_bytes(&path);
                sizes.push((name, size));
            }
        }
    }
    sizes.sort_by(|a, b| a.0.cmp(&b.0));
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn file_size_bytes_returns_correct_size() {
        let dir = std::env::temp_dir().join("molt-harness-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.bin");
        fs::write(&path, b"hello").unwrap();
        assert_eq!(file_size_bytes(&path), 5);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn file_size_bytes_returns_zero_for_missing() {
        let path = Path::new("/nonexistent/file.bin");
        assert_eq!(file_size_bytes(path), 0);
    }

    #[test]
    fn artifact_sizes_finds_matching_files() {
        let dir = std::env::temp_dir().join("molt-harness-artifacts");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.wasm"), b"abc").unwrap();
        fs::write(dir.join("b.wasm"), b"abcdef").unwrap();
        fs::write(dir.join("c.txt"), b"ignored").unwrap();

        let sizes = artifact_sizes(&dir, "wasm");
        assert_eq!(sizes.len(), 2);
        assert_eq!(sizes[0], ("a.wasm".to_string(), 3));
        assert_eq!(sizes[1], ("b.wasm".to_string(), 6));

        fs::remove_dir_all(&dir).unwrap();
    }
}
```

- [ ] **Step 4: Add to workspace**

Read `Cargo.toml` (workspace root) and add `"runtime/molt-harness"` to the `members` list.

- [ ] **Step 5: Verify compilation and tests**

Run: `cargo check -p molt-harness && cargo test -p molt-harness`
Expected: 3 tests PASS

- [ ] **Step 6: Commit**

```bash
git add runtime/molt-harness/ Cargo.toml
git commit -m "feat(harness): add molt-harness Rust crate with size_tracking module"
```

---

### Task 8: Harness Self-Tests

**Files:**
- Create: `tests/test_harness_self.py`

- [ ] **Step 1: Write self-tests for the harness**

`tests/test_harness_self.py`:
```python
"""Self-tests for the harness infrastructure.

These verify that the harness itself works correctly — profile definitions,
layer execution, baseline ratcheting, report generation.
"""
import json
import sys
sys.path.insert(0, "src")

from molt.harness_layers import LAYERS, PROFILES, get_layers_for_profile
from molt.harness_report import Baseline, HarnessReport, LayerResult, LayerStatus


def test_all_layers_have_unique_names():
    names = [l.name for l in LAYERS]
    assert len(names) == len(set(names)), f"duplicate layer names: {names}"


def test_profiles_reference_only_existing_layers():
    layer_names = {l.name for l in LAYERS}
    for profile, names in PROFILES.items():
        for name in names:
            assert name in layer_names, f"profile {profile!r} references unknown layer {name!r}"


def test_profiles_are_strict_supersets():
    quick = PROFILES["quick"]
    standard = PROFILES["standard"]
    deep = PROFILES["deep"]
    assert quick == standard[:len(quick)], "standard must start with all quick layers"
    assert standard == deep[:len(standard)], "deep must start with all standard layers"


def test_layer_count_matches_spec():
    """The spec defines exactly 16 layers."""
    assert len(LAYERS) == 16, f"expected 16 layers, got {len(LAYERS)}"


def test_quick_profile_has_4_layers():
    assert len(PROFILES["quick"]) == 4


def test_standard_profile_has_8_layers():
    assert len(PROFILES["standard"]) == 8


def test_deep_profile_has_16_layers():
    assert len(PROFILES["deep"]) == 16


def test_baseline_json_schema(tmp_path):
    b = Baseline(test_counts={"unit-rust": 40}, metrics={"fib_30_ns": 12345.0})
    path = tmp_path / "b.json"
    b.save(path)
    data = json.loads(path.read_text())
    assert "test_counts" in data
    assert "metrics" in data
    assert isinstance(data["test_counts"], dict)
    assert isinstance(data["metrics"], dict)


def test_report_json_has_required_fields():
    report = HarnessReport(profile="quick", results=[
        LayerResult(name="compile", status=LayerStatus.PASS, duration_s=1.0),
    ])
    data = json.loads(report.to_json())
    required = {"profile", "timestamp", "all_passed", "total_duration_s",
                "pass_count", "fail_count", "results"}
    assert required.issubset(set(data.keys())), f"missing keys: {required - set(data.keys())}"

    result = data["results"][0]
    result_required = {"name", "status", "duration_s", "details", "metrics"}
    assert result_required.issubset(set(result.keys()))
```

- [ ] **Step 2: Run tests**

Run: `PYTHONPATH=src python3 -m pytest tests/test_harness_self.py -v`
Expected: All 10 tests PASS

- [ ] **Step 3: Commit**

```bash
git add tests/test_harness_self.py
git commit -m "test(harness): add self-tests verifying harness infrastructure correctness"
```

---

## Self-Review

**Spec coverage check:**
- Section 1 (Goal): Covered by Task 4 orchestrator
- Section 3 (Principles): local-first (Task 3/4), zero tolerance (Task 3 gates), ratchet (Task 2), fast feedback (Task 3 profiles), evidence-based (Task 3 fresh execution)
- Section 4 (Architecture): Python orchestrator (Task 4), Rust library (Task 7), test corpus (Task 5/6)
- Section 5.1 (Layer order): Task 3 — all 16 layers registered in order
- Section 5.2 (Feature-flag matrix): Task 3 `run_layer_unit_rust` runs 3 modes
- Section 5.4 (Resource scenarios): Task 5 — all 6 scenario files
- Section 5.5 (Fuzz): Layers defined as stubs in Task 3, fuzz targets already exist from prior sprint
- Section 5.6 (Combinatorial): Task 6 — Jinja2 template
- Section 6 (Gates): Task 2 baseline + Task 3 zero-failure checks
- Section 7 (File layout): Matches Tasks 1-7 file paths
- Section 8 (Prior art): Patterns wired into layer implementations
- Section 9 (Self-tests): Task 8
- Section 10 (Integration): Task 4 CLI integration
- **Gap: standard/deep layer implementations are stubs.** This is intentional — the plan covers the framework; each layer fills in as the corresponding feature matures. The stubs produce SKIP results that don't block.

**Placeholder scan:** No TBD/TODO. All code blocks are complete. Stub layers are explicit `_stub_layer` functions that return SKIP, not empty implementations.

**Type consistency:** `LayerResult`, `HarnessReport`, `Baseline`, `LayerDef`, `HarnessConfig` used consistently across all tasks. `LayerStatus.PASS/FAIL/SKIP` enum used everywhere.
