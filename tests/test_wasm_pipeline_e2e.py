"""End-to-end WASM pipeline test.

Exercises the full compilation pipeline:
  1. Compile hello.py to a linked .wasm module
  2. Optimize with wasm-opt (if available)
  3. Run with wasmtime (if available)
  4. Report sizes at each stage
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest
from tests.wasm_linked_runner import (
    _run_wasm_test_process,
    selected_wasm_ld,
    wasm_ld_unavailable_reason,
    wasm_test_build_env,
)
from tools import wasm_optimize

ROOT = Path(__file__).resolve().parents[1]
HELLO_PY = ROOT / "examples" / "hello.py"
WASM_LD_RUSTUP_GLOB = "toolchains/stable-*/lib/rustlib/*/bin/gcc-ld/wasm-ld"


def _find_wasmtime() -> str | None:
    return shutil.which("wasmtime")


def _molt_build(
    source: Path,
    output_dir: Path,
    *,
    extra_env: dict[str, str] | None = None,
) -> Path | None:
    """Run a linked `molt build` and return the module's path, or None on failure."""
    env = wasm_test_build_env(ROOT, linked=True)
    if extra_env:
        env.update(extra_env)
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(source),
        "--target",
        "wasm",
        "--no-cache",
        # The dev profile runs no wasm-opt, so the optimizer stage below has
        # an unoptimized module to reduce.
        "--build-profile",
        "dev",
        "--require-linked",
        "--out-dir",
        str(output_dir),
    ]
    result = _run_wasm_test_process(
        cmd,
        role="tool",
        env=env,
        cwd=ROOT,
        timeout=300,
    )
    if result.returncode != 0:
        return None
    # Find the output wasm
    candidate = output_dir / "output_linked.wasm"
    if candidate.exists():
        return candidate
    return None


def _wasm_opt(input_path: Path, output_path: Path) -> dict[str, object]:
    """Run the canonical Oz optimizer and return its typed result."""
    return wasm_optimize.optimize(
        input_path,
        output_path=output_path,
        level="Oz",
        extra_passes=("--strip-debug",),
        timeout=120,
    )


def _wasmtime_run(wasm_path: Path) -> tuple[bool, str]:
    """Run wasm with wasmtime, return (success, stdout)."""
    wasmtime = _find_wasmtime()
    if not wasmtime:
        return False, "wasmtime not found"
    result = _run_wasm_test_process(
        [wasmtime, str(wasm_path)],
        role="tool",
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    return result.returncode == 0, result.stdout.strip()


def test_wasm_pipeline_molt_build_uses_guarded_process(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    source = tmp_path / "hello.py"
    source.write_text("print(42)\n", encoding="utf-8")
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    captured: dict[str, object] = {}

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = list(cmd)
        captured["kwargs"] = kwargs
        (out_dir / "output_linked.wasm").write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr(
        sys.modules[__name__],
        "_run_wasm_test_process",
        fake_run,
    )

    output = _molt_build(source, out_dir)

    assert output == out_dir / "output_linked.wasm"
    assert captured["cmd"][:4] == [sys.executable, "-m", "molt.cli", "build"]
    assert captured["kwargs"]["cwd"] == ROOT
    assert captured["kwargs"]["timeout"] == 300


def test_wasm_pipeline_external_tools_use_guarded_process(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    input_wasm = tmp_path / "in.wasm"
    output_wasm = tmp_path / "out.wasm"
    input_wasm.write_bytes(b"\x00asm\x01\x00\x00\x00")
    process_calls: list[list[str]] = []
    optimizer_calls: list[tuple[Path, dict[str, object]]] = []

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        process_calls.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, stdout="42\n", stderr="")

    def fake_optimize(input_path, **kwargs):  # type: ignore[no-untyped-def]
        optimizer_calls.append((input_path, kwargs))
        output_wasm.write_bytes(b"\x00asm")
        return {"ok": True}

    monkeypatch.setattr(wasm_optimize, "optimize", fake_optimize)
    monkeypatch.setattr(sys.modules[__name__], "_find_wasmtime", lambda: "wasmtime")
    monkeypatch.setattr(
        sys.modules[__name__],
        "_run_wasm_test_process",
        fake_run,
    )

    assert _wasm_opt(input_wasm, output_wasm)["ok"] is True
    assert _wasmtime_run(output_wasm) == (True, "42")
    assert optimizer_calls == [
        (
            input_wasm,
            {
                "output_path": output_wasm,
                "level": "Oz",
                "extra_passes": ("--strip-debug",),
                "timeout": 120,
            },
        )
    ]
    assert process_calls[0][0] == "wasmtime"


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def pipeline_results() -> dict:
    """Run the full pipeline once and cache results for all tests."""
    results: dict = {"stages": {}}
    with tempfile.TemporaryDirectory(prefix="molt-e2e-") as tmpdir:
        tmpdir_path = Path(tmpdir)

        # Stage 1: Linked build
        if wasm_ld_unavailable_reason() is None:
            linked_dir = tmpdir_path / "linked"
            linked_dir.mkdir()
            linked = _molt_build(HELLO_PY, linked_dir)
            if linked and linked.exists():
                results["stages"]["linked"] = {
                    "path": linked,
                    "size": linked.stat().st_size,
                }

        optimizer_available = wasm_optimize.find_wasm_opt() is not None

        # Stage 2: wasm-opt on linked
        if "linked" in results["stages"] and optimizer_available:
            linked_opt = tmpdir_path / "linked_optimized.wasm"
            src = results["stages"]["linked"]["path"]
            optimize_result = _wasm_opt(src, linked_opt)
            assert optimize_result["ok"], optimize_result["error"]
            assert linked_opt.exists(), "canonical optimizer published no artifact"
            results["stages"]["linked_optimized"] = {
                "path": linked_opt,
                "size": linked_opt.stat().st_size,
            }

        # Report
        print("\n=== WASM Pipeline Size Report ===")
        for stage, info in results["stages"].items():
            size = info["size"]
            print(f"  {stage:<25s} {size:>12,} bytes ({size / 1024 / 1024:.2f} MB)")

        if "linked" in results["stages"] and "linked_optimized" in results["stages"]:
            orig = results["stages"]["linked"]["size"]
            opt = results["stages"]["linked_optimized"]["size"]
            print(f"  linked->optimized: {(orig - opt) / orig * 100:.1f}% reduction")

        # Copy results before tmpdir cleanup
        for stage, info in results["stages"].items():
            info["size_bytes"] = info["size"]
            del info["path"]

    return results


class TestWasmPipelineE2E:
    """End-to-end WASM pipeline tests."""

    def test_wasm_ld_detected(self, pipeline_results: dict) -> None:
        assert selected_wasm_ld().is_file()

    def test_linked_build_succeeds(self, pipeline_results: dict) -> None:
        selected_wasm_ld()
        assert "linked" in pipeline_results["stages"], "Linked WASM build failed"

    def test_wasm_opt_reduces_size(self, pipeline_results: dict) -> None:
        stages = pipeline_results["stages"]
        if "linked" not in stages or "linked_optimized" not in stages:
            pytest.skip("wasm-opt optimization not available")
        orig = stages["linked"]["size_bytes"]
        opt = stages["linked_optimized"]["size_bytes"]
        reduction_pct = (orig - opt) / orig * 100
        assert reduction_pct > 10, (
            f"wasm-opt only achieved {reduction_pct:.1f}% reduction "
            f"({orig:,} -> {opt:,})"
        )
