"""Harness layer definitions and implementations.

Each layer is a self-contained verification step that produces a LayerResult.
Layers are grouped into profiles (quick, standard, deep) where each profile
is a strict superset of the previous one.
"""
from __future__ import annotations

import re
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

from molt.harness_report import LayerResult, LayerStatus


@dataclass
class HarnessConfig:
    """Configuration for a harness run."""

    project_root: Path
    fail_fast: bool = False
    fuzz_duration_s: int = 30
    molt_cmd: str = "molt"
    verbose: bool = False

    @property
    def molt_crates(self) -> list[str]:
        """Return the list of molt workspace crate directory names."""
        runtime_dir = self.project_root / "runtime"
        if not runtime_dir.is_dir():
            return []
        return sorted(
            d.name
            for d in runtime_dir.iterdir()
            if d.is_dir() and (d / "Cargo.toml").exists()
        )


@dataclass
class LayerDef:
    """Definition of a single harness layer."""

    name: str
    profile: str  # "quick", "standard", or "deep"
    run_fn: Callable[[HarnessConfig], LayerResult]


def _run_cmd(
    args: list[str],
    *,
    cwd: Path | None = None,
    timeout_s: int = 300,
) -> subprocess.CompletedProcess[str]:
    """Run a subprocess, handling common failure modes."""
    try:
        return subprocess.run(
            args,
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=timeout_s,
        )
    except FileNotFoundError as exc:
        return subprocess.CompletedProcess(
            args=args,
            returncode=127,
            stdout="",
            stderr=f"command not found: {exc}",
        )
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(
            args=args,
            returncode=124,
            stdout="",
            stderr=f"command timed out after {timeout_s}s",
        )


# ---------------------------------------------------------------------------
# Quick-profile layer implementations
# ---------------------------------------------------------------------------


def run_layer_compile(config: HarnessConfig) -> LayerResult:
    """Run ``cargo check`` on all molt crates. Fails on errors or warnings."""
    t0 = time.monotonic()
    proc = _run_cmd(
        ["cargo", "check", "--workspace", "--message-format=short"],
        cwd=config.project_root / "runtime",
    )
    elapsed = time.monotonic() - t0

    combined = proc.stdout + proc.stderr
    has_warnings = "warning" in combined.lower() and "generated" in combined.lower()
    if proc.returncode != 0 or has_warnings:
        return LayerResult(
            name="compile",
            status=LayerStatus.FAIL,
            duration_s=elapsed,
            details=combined[-500:] if combined else "cargo check failed",
        )
    return LayerResult(
        name="compile",
        status=LayerStatus.PASS,
        duration_s=elapsed,
        details="clean compile, no warnings",
    )


def run_layer_lint(config: HarnessConfig) -> LayerResult:
    """Run ``cargo clippy -D warnings`` on all molt crates."""
    t0 = time.monotonic()
    proc = _run_cmd(
        ["cargo", "clippy", "--workspace", "--", "-D", "warnings"],
        cwd=config.project_root / "runtime",
    )
    elapsed = time.monotonic() - t0

    if proc.returncode != 0:
        combined = proc.stdout + proc.stderr
        return LayerResult(
            name="lint",
            status=LayerStatus.FAIL,
            duration_s=elapsed,
            details=combined[-500:] if combined else "clippy failed",
        )
    return LayerResult(
        name="lint",
        status=LayerStatus.PASS,
        duration_s=elapsed,
        details="clippy clean",
    )


_TEST_RESULT_RE = re.compile(r"test result: \w+\. (\d+) passed")


def run_layer_unit_rust(config: HarnessConfig) -> LayerResult:
    """Run ``cargo test`` in three feature-flag modes.

    Modes: default features, ``refcount_verify``, and ``audit``.
    Parses "test result:" lines to count total passed tests.
    """
    t0 = time.monotonic()
    feature_modes: list[list[str]] = [
        ["cargo", "test", "--workspace"],
        ["cargo", "test", "--workspace", "--features", "refcount_verify"],
        ["cargo", "test", "--workspace", "--features", "audit"],
    ]
    total_passed = 0
    failures: list[str] = []

    for cmd in feature_modes:
        proc = _run_cmd(cmd, cwd=config.project_root / "runtime", timeout_s=600)
        if proc.returncode != 0:
            label = " ".join(cmd[3:]) or "default"
            failures.append(f"{label}: rc={proc.returncode}")
        # Parse passed counts from all "test result:" lines
        for match in _TEST_RESULT_RE.finditer(proc.stdout):
            total_passed += int(match.group(1))

    elapsed = time.monotonic() - t0
    if failures:
        return LayerResult(
            name="unit-rust",
            status=LayerStatus.FAIL,
            duration_s=elapsed,
            details="; ".join(failures),
            metrics={"tests_passed": total_passed},
        )
    return LayerResult(
        name="unit-rust",
        status=LayerStatus.PASS,
        duration_s=elapsed,
        details=f"{total_passed} tests passed across 3 modes",
        metrics={"tests_passed": total_passed},
    )


def run_layer_unit_python(config: HarnessConfig) -> LayerResult:
    """Run Python-side checks: capability manifest + REPL import verification."""
    t0 = time.monotonic()
    errors: list[str] = []

    # 1. Run capability manifest
    proc = _run_cmd(
        ["python3", "-m", "molt.capability_manifest"],
        cwd=config.project_root,
    )
    if proc.returncode != 0:
        errors.append(f"capability_manifest: {proc.stderr[:200]}")

    # 2. Verify REPL import
    proc = _run_cmd(
        ["python3", "-c", "import molt; print('ok')"],
        cwd=config.project_root,
    )
    if proc.returncode != 0:
        errors.append(f"repl import: {proc.stderr[:200]}")

    elapsed = time.monotonic() - t0
    if errors:
        return LayerResult(
            name="unit-python",
            status=LayerStatus.FAIL,
            duration_s=elapsed,
            details="; ".join(errors),
        )
    return LayerResult(
        name="unit-python",
        status=LayerStatus.PASS,
        duration_s=elapsed,
        details="manifest + import ok",
    )


# ---------------------------------------------------------------------------
# Stub for not-yet-implemented layers
# ---------------------------------------------------------------------------


def _stub_layer(name: str) -> Callable[[HarnessConfig], LayerResult]:
    """Return a function that produces a SKIP result for unimplemented layers."""

    def _run(config: HarnessConfig) -> LayerResult:
        return LayerResult(
            name=name,
            status=LayerStatus.SKIP,
            duration_s=0.0,
            details="not yet implemented",
        )

    _run.__qualname__ = f"stub_{name}"
    return _run


# ---------------------------------------------------------------------------
# Layer registry
# ---------------------------------------------------------------------------

LAYERS: list[LayerDef] = [
    # quick (4 layers)
    LayerDef(name="compile", profile="quick", run_fn=run_layer_compile),
    LayerDef(name="lint", profile="quick", run_fn=run_layer_lint),
    LayerDef(name="unit-rust", profile="quick", run_fn=run_layer_unit_rust),
    LayerDef(name="unit-python", profile="quick", run_fn=run_layer_unit_python),
    # standard (4 additional layers)
    LayerDef(name="wasm-compile", profile="standard", run_fn=_stub_layer("wasm-compile")),
    LayerDef(name="differential", profile="standard", run_fn=_stub_layer("differential")),
    LayerDef(name="resource", profile="standard", run_fn=_stub_layer("resource")),
    LayerDef(name="audit", profile="standard", run_fn=_stub_layer("audit")),
    # deep (8 additional layers)
    LayerDef(name="fuzz", profile="deep", run_fn=_stub_layer("fuzz")),
    LayerDef(name="conformance", profile="deep", run_fn=_stub_layer("conformance")),
    LayerDef(name="bench", profile="deep", run_fn=_stub_layer("bench")),
    LayerDef(name="size", profile="deep", run_fn=_stub_layer("size")),
    LayerDef(name="mutation", profile="deep", run_fn=_stub_layer("mutation")),
    LayerDef(name="determinism", profile="deep", run_fn=_stub_layer("determinism")),
    LayerDef(name="miri", profile="deep", run_fn=_stub_layer("miri")),
    LayerDef(name="compile-fail", profile="deep", run_fn=_stub_layer("compile-fail")),
]

assert len(LAYERS) == 16, f"expected 16 layers, got {len(LAYERS)}"

# Profile definitions — each is a strict superset of the previous.
_QUICK_NAMES = [l.name for l in LAYERS if l.profile == "quick"]
_STANDARD_NAMES = _QUICK_NAMES + [l.name for l in LAYERS if l.profile == "standard"]
_DEEP_NAMES = _STANDARD_NAMES + [l.name for l in LAYERS if l.profile == "deep"]

PROFILES: dict[str, list[str]] = {
    "quick": _QUICK_NAMES,
    "standard": _STANDARD_NAMES,
    "deep": _DEEP_NAMES,
}

_LAYER_INDEX: dict[str, LayerDef] = {l.name: l for l in LAYERS}


def get_layers_for_profile(profile: str) -> list[LayerDef]:
    """Return an ordered list of LayerDefs for the given profile."""
    if profile not in PROFILES:
        raise ValueError(f"unknown profile: {profile!r} (choose from {list(PROFILES)})")
    return [_LAYER_INDEX[name] for name in PROFILES[profile]]
