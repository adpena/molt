"""Tests for tracking wasm binary sizes across builds (regression tracking)."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).resolve().parents[1]
FIXTURE = PROJECT_ROOT / "tests" / "fixtures" / "freestanding_hello.py"

# 16 MB -- matches the default total budget in tools/wasm_size_audit.py
SIZE_BUDGET_BYTES = 16 * 1024 * 1024


def _build_and_measure(src_path: Path, tmp_path: Path, target: str = "wasm") -> dict:
    """Build a molt program to wasm and return size metrics."""
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt",
            "build",
            str(src_path),
            "--target",
            target,
            "--output",
            str(output),
            "--linked-output",
            str(linked),
        ],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT,
        timeout=180,
    )
    if result.returncode != 0:
        return {"ok": False, "error": result.stderr}

    sizes: dict = {"ok": True}
    if output.exists():
        sizes["unlinked_bytes"] = output.stat().st_size
    if linked.exists():
        sizes["linked_bytes"] = linked.stat().st_size
    return sizes


@pytest.mark.slow
def test_hello_world_size_budget(tmp_path: Path) -> None:
    """The linked hello-world wasm binary must stay under the size budget."""
    sizes = _build_and_measure(FIXTURE, tmp_path, target="wasm")
    assert sizes["ok"], f"Build failed: {sizes.get('error', '(unknown)')}"
    assert "linked_bytes" in sizes, "No linked binary produced"

    linked_bytes = sizes["linked_bytes"]
    linked_kb = linked_bytes / 1024
    print(
        f"\n[size-budget] linked binary: {linked_bytes} bytes ({linked_kb:.1f} KB)",
        file=sys.stderr,
    )
    assert linked_bytes <= SIZE_BUDGET_BYTES, (
        f"Linked binary {linked_bytes} bytes ({linked_kb:.1f} KB) "
        f"exceeds budget of {SIZE_BUDGET_BYTES} bytes "
        f"({SIZE_BUDGET_BYTES / 1024:.1f} KB)"
    )


@pytest.mark.slow
def test_freestanding_smaller_than_wasi(tmp_path: Path) -> None:
    """A freestanding build should be smaller than a full WASI build."""
    wasi_dir = tmp_path / "wasi"
    wasi_dir.mkdir()
    free_dir = tmp_path / "freestanding"
    free_dir.mkdir()

    wasi_sizes = _build_and_measure(FIXTURE, wasi_dir, target="wasm")
    free_sizes = _build_and_measure(FIXTURE, free_dir, target="wasm-freestanding")

    assert wasi_sizes["ok"], f"WASI build failed: {wasi_sizes.get('error', '')}"
    assert free_sizes["ok"], f"Freestanding build failed: {free_sizes.get('error', '')}"
    assert "linked_bytes" in wasi_sizes, "No linked WASI binary produced"
    assert "linked_bytes" in free_sizes, "No linked freestanding binary produced"

    wasi_bytes = wasi_sizes["linked_bytes"]
    free_bytes = free_sizes["linked_bytes"]
    delta = wasi_bytes - free_bytes

    print(
        f"\n[size-compare] WASI: {wasi_bytes} bytes, "
        f"freestanding: {free_bytes} bytes, "
        f"delta: {delta} bytes",
        file=sys.stderr,
    )
    assert free_bytes < wasi_bytes, (
        f"Freestanding binary ({free_bytes} bytes) is not smaller than "
        f"WASI binary ({wasi_bytes} bytes)"
    )


@pytest.mark.slow
def test_size_report(tmp_path: Path) -> None:
    """Print a structured size report for CI visibility (always passes)."""
    sizes = _build_and_measure(FIXTURE, tmp_path, target="wasm")
    if not sizes["ok"]:
        print(
            f"\n[size-report] Build failed, skipping report: {sizes.get('error', '')}",
            file=sys.stderr,
        )
        return

    unlinked = sizes.get("unlinked_bytes", 0)
    linked = sizes.get("linked_bytes", 0)
    delta = linked - unlinked
    pct = (delta / unlinked * 100) if unlinked > 0 else 0.0

    report = (
        f"\nWASM Size Report:\n"
        f"  Unlinked: {unlinked} bytes ({unlinked / 1024:.1f} KB)\n"
        f"  Linked:   {linked} bytes ({linked / 1024:.1f} KB)\n"
        f"  Delta:    {delta} bytes ({pct:.1f}% overhead from linking)"
    )
    print(report, file=sys.stderr)
