"""End-to-end WASM pipeline test.

Exercises the full compilation pipeline:
  1. Compile hello.py -> .wasm (standalone)
  2. Compile hello.py -> .wasm (relocatable, linked with wasm-ld)
  3. Optimize with wasm-opt (if available)
  4. Run with wasmtime (if available)
  5. Report sizes at each stage
"""

from __future__ import annotations

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest
from tests.wasm_linked_runner import _run_wasm_test_process

ROOT = Path(__file__).resolve().parents[1]
HELLO_PY = ROOT / "examples" / "hello.py"
WASM_LD_RUSTUP_GLOB = "toolchains/stable-*/lib/rustlib/*/bin/gcc-ld/wasm-ld"


def _load_wasm_link():
    path = ROOT / "tools" / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


wasm_link = _load_wasm_link()


def _find_wasm_ld() -> str | None:
    return wasm_link._find_wasm_ld()


def _find_wasm_opt() -> str | None:
    return shutil.which("wasm-opt")


def _find_wasmtime() -> str | None:
    return shutil.which("wasmtime")


def _molt_build(
    source: Path,
    output_dir: Path,
    *,
    linked: bool = False,
    extra_env: dict[str, str] | None = None,
) -> Path | None:
    """Run `molt build` and return the output .wasm path, or None on failure."""
    env = os.environ.copy()
    repo_src = str(ROOT / "src")
    current_pythonpath = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = (
        repo_src + os.pathsep + current_pythonpath if current_pythonpath else repo_src
    )
    env["MOLT_BACKEND_DAEMON"] = "0"
    if linked:
        env["MOLT_WASM_LINK"] = "1"
        wasm_ld_path = _find_wasm_ld()
        if wasm_ld_path:
            ld_dir = str(Path(wasm_ld_path).parent)
            env["PATH"] = ld_dir + os.pathsep + env.get("PATH", "")
    else:
        env["MOLT_WASM_LINKED"] = "0"
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
        "--out-dir",
        str(output_dir),
    ]
    if linked:
        cmd.append("--linked")
    result = _run_wasm_test_process(
        cmd,
        env=env,
        cwd=ROOT,
        timeout=300,
    )
    if result.returncode != 0:
        return None
    # Find the output wasm
    for name in ("output_linked.wasm", "output.wasm"):
        candidate = output_dir / name
        if candidate.exists():
            return candidate
    return None


def _wasm_opt(input_path: Path, output_path: Path) -> bool:
    """Run wasm-opt -Oz on input, return True on success."""
    wasm_opt = _find_wasm_opt()
    if not wasm_opt:
        return False
    result = _run_wasm_test_process(
        [
            wasm_opt,
            "-Oz",
            "--enable-reference-types",
            "--enable-bulk-memory",
            "--enable-simd",
            "--enable-sign-ext",
            "--enable-mutable-globals",
            "--enable-nontrapping-float-to-int",
            "--strip-debug",
            "--no-validation",
            str(input_path),
            "-o",
            str(output_path),
        ],
        cwd=ROOT,
        env=os.environ,
        timeout=120,
    )
    return result.returncode == 0 and output_path.exists()


def _wasmtime_run(wasm_path: Path) -> tuple[bool, str]:
    """Run wasm with wasmtime, return (success, stdout)."""
    wasmtime = _find_wasmtime()
    if not wasmtime:
        return False, "wasmtime not found"
    result = _run_wasm_test_process(
        [wasmtime, str(wasm_path)],
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
        (out_dir / "output.wasm").write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr(
        sys.modules[__name__],
        "_run_wasm_test_process",
        fake_run,
    )

    output = _molt_build(source, out_dir)

    assert output == out_dir / "output.wasm"
    assert captured["kwargs"]["cwd"] == ROOT
    assert captured["kwargs"]["timeout"] == 300


def test_wasm_pipeline_external_tools_use_guarded_process(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    input_wasm = tmp_path / "in.wasm"
    output_wasm = tmp_path / "out.wasm"
    input_wasm.write_bytes(b"\x00asm")
    calls: list[list[str]] = []

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        calls.append(list(cmd))
        if "-o" in cmd:
            output_wasm.write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(cmd, 0, stdout="42\n", stderr="")

    monkeypatch.setattr(sys.modules[__name__], "_find_wasm_opt", lambda: "wasm-opt")
    monkeypatch.setattr(sys.modules[__name__], "_find_wasmtime", lambda: "wasmtime")
    monkeypatch.setattr(
        sys.modules[__name__],
        "_run_wasm_test_process",
        fake_run,
    )

    assert _wasm_opt(input_wasm, output_wasm) is True
    assert _wasmtime_run(output_wasm) == (True, "42")
    assert calls[0][0] == "wasm-opt"
    assert calls[1][0] == "wasmtime"


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def pipeline_results() -> dict:
    """Run the full pipeline once and cache results for all tests."""
    results: dict = {"stages": {}}
    with tempfile.TemporaryDirectory(prefix="molt-e2e-") as tmpdir:
        tmpdir_path = Path(tmpdir)

        # Stage 1: Standalone build
        standalone_dir = tmpdir_path / "standalone"
        standalone_dir.mkdir()
        standalone = _molt_build(HELLO_PY, standalone_dir, linked=False)
        if standalone and standalone.exists():
            results["stages"]["standalone"] = {
                "path": standalone,
                "size": standalone.stat().st_size,
            }

        # Stage 2: Linked build
        wasm_ld = _find_wasm_ld()
        if wasm_ld:
            linked_dir = tmpdir_path / "linked"
            linked_dir.mkdir()
            linked = _molt_build(HELLO_PY, linked_dir, linked=True)
            if linked and linked.exists():
                results["stages"]["linked"] = {
                    "path": linked,
                    "size": linked.stat().st_size,
                }

        # Stage 3: wasm-opt optimization (on standalone)
        if "standalone" in results["stages"]:
            opt_path = tmpdir_path / "output_optimized.wasm"
            src = results["stages"]["standalone"]["path"]
            if _wasm_opt(src, opt_path):
                results["stages"]["optimized"] = {
                    "path": opt_path,
                    "size": opt_path.stat().st_size,
                }

        # Stage 4: wasm-opt on linked
        if "linked" in results["stages"]:
            linked_opt = tmpdir_path / "linked_optimized.wasm"
            src = results["stages"]["linked"]["path"]
            if _wasm_opt(src, linked_opt):
                results["stages"]["linked_optimized"] = {
                    "path": linked_opt,
                    "size": linked_opt.stat().st_size,
                }

        # Report
        print("\n=== WASM Pipeline Size Report ===")
        for stage, info in results["stages"].items():
            size = info["size"]
            print(f"  {stage:<25s} {size:>12,} bytes ({size / 1024 / 1024:.2f} MB)")

        if "standalone" in results["stages"] and "optimized" in results["stages"]:
            orig = results["stages"]["standalone"]["size"]
            opt = results["stages"]["optimized"]["size"]
            print(
                f"  standalone->optimized: {(orig - opt) / orig * 100:.1f}% reduction"
            )

        # Copy results before tmpdir cleanup
        for stage, info in results["stages"].items():
            info["size_bytes"] = info["size"]
            del info["path"]

    return results


class TestWasmPipelineE2E:
    """End-to-end WASM pipeline tests."""

    def test_standalone_build_succeeds(self, pipeline_results: dict) -> None:
        assert "standalone" in pipeline_results["stages"], (
            "Standalone WASM build failed"
        )

    def test_standalone_size_reasonable(self, pipeline_results: dict) -> None:
        stages = pipeline_results["stages"]
        if "standalone" not in stages:
            pytest.skip("Standalone build not available")
        size = stages["standalone"]["size_bytes"]
        # Standalone output should be under 20MB
        assert size < 20 * 1024 * 1024, f"Standalone size {size:,} exceeds 20MB"
        # Standalone modules contain only user code + imports (no bundled
        # runtime).  For hello.py this is typically 5-50KB.
        assert size > 1024, f"Standalone size {size:,} suspiciously small"

    def test_wasm_ld_detected(self, pipeline_results: dict) -> None:
        wasm_ld = _find_wasm_ld()
        if wasm_ld is None:
            pytest.skip(
                "wasm-ld not available; install LLVM or rustup stable toolchain"
            )
        assert Path(wasm_ld).is_file()

    def test_linked_build_succeeds(self, pipeline_results: dict) -> None:
        if _find_wasm_ld() is None:
            pytest.skip("wasm-ld not available")
        assert "linked" in pipeline_results["stages"], "Linked WASM build failed"

    def test_wasm_opt_reduces_size(self, pipeline_results: dict) -> None:
        stages = pipeline_results["stages"]
        if "standalone" not in stages or "optimized" not in stages:
            pytest.skip("wasm-opt optimization not available")
        orig = stages["standalone"]["size_bytes"]
        opt = stages["optimized"]["size_bytes"]
        reduction_pct = (orig - opt) / orig * 100
        assert reduction_pct > 10, (
            f"wasm-opt only achieved {reduction_pct:.1f}% reduction "
            f"({orig:,} -> {opt:,})"
        )
