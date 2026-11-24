"""Tests for tracking wasm binary sizes across builds (regression tracking)."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

from tests.wasm_linked_runner import _run_wasm_test_process, wasm_test_build_env

PROJECT_ROOT = Path(__file__).resolve().parents[1]
FIXTURE = PROJECT_ROOT / "tests" / "fixtures" / "freestanding_hello.py"

# 16 MB -- matches the default total budget in tools/wasm_size_audit.py
SIZE_BUDGET_BYTES = 16 * 1024 * 1024


def _build_and_measure(src_path: Path, tmp_path: Path, target: str = "wasm") -> dict:
    """Build a molt program to wasm and return size metrics."""
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    result = _run_wasm_test_process(
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
        role="tool",
        cwd=PROJECT_ROOT,
        env=wasm_test_build_env(PROJECT_ROOT, linked=True),
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


def test_build_and_measure_uses_wasm_test_guard(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    captured: dict[str, object] = {}

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = list(cmd)
        captured["kwargs"] = kwargs
        output = tmp_path / "output.wasm"
        linked = tmp_path / "output_linked.wasm"
        output.write_bytes(b"\x00asm")
        linked.write_bytes(b"\x00asm-linked")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr(sys.modules[__name__], "_run_wasm_test_process", fake_run)

    sizes = _build_and_measure(FIXTURE, tmp_path)

    assert sizes["ok"] is True
    assert sizes["linked_bytes"] == len(b"\x00asm-linked")
    assert captured["kwargs"]["cwd"] == PROJECT_ROOT
    assert captured["kwargs"]["timeout"] == 180


@pytest.mark.slow
def test_hello_world_size_budget_and_freestanding_size(tmp_path: Path) -> None:
    """Check the WASI size budget and compare the freestanding build."""
    wasi_dir = tmp_path / "wasi"
    wasi_dir.mkdir()
    sizes = _build_and_measure(FIXTURE, wasi_dir, target="wasm")
    assert sizes["ok"], f"Build failed: {sizes.get('error', '(unknown)')}"
    assert "linked_bytes" in sizes, "No linked binary produced"

    linked_bytes = sizes["linked_bytes"]
    unlinked_bytes = sizes.get("unlinked_bytes", 0)
    delta = linked_bytes - unlinked_bytes
    print(
        f"\n[size-budget] linked binary: {linked_bytes} bytes "
        f"({linked_bytes / 1024:.1f} KiB), unlinked: {unlinked_bytes} bytes, "
        f"link delta: {delta:+d} bytes",
        file=sys.stderr,
    )
    assert linked_bytes <= SIZE_BUDGET_BYTES, (
        f"Linked binary {linked_bytes} bytes ({linked_bytes / 1024:.1f} KiB) "
        f"exceeds budget of {SIZE_BUDGET_BYTES} bytes "
        f"({SIZE_BUDGET_BYTES / 1024:.1f} KB)"
    )

    free_dir = tmp_path / "freestanding"
    free_dir.mkdir()
    free_sizes = _build_and_measure(FIXTURE, free_dir, target="wasm-freestanding")

    assert free_sizes["ok"], f"Freestanding build failed: {free_sizes.get('error', '')}"
    assert "linked_bytes" in free_sizes, "No linked freestanding binary produced"

    free_bytes = free_sizes["linked_bytes"]
    delta = linked_bytes - free_bytes

    print(
        f"\n[size-compare] WASI: {linked_bytes} bytes, "
        f"freestanding: {free_bytes} bytes, "
        f"delta: {delta} bytes",
        file=sys.stderr,
    )
    assert free_bytes < linked_bytes, (
        f"Freestanding binary ({free_bytes} bytes) is not smaller than "
        f"WASI binary ({linked_bytes} bytes)"
    )
