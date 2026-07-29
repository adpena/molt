"""WASM performance and size threshold tests.

Validates that compiled WASM output stays within expected size budgets
and that wasm-ld / wasm-opt tooling is available for the optimization
pipeline.
"""

from __future__ import annotations

import importlib.util
import os
import shutil
import sys
import tempfile
from pathlib import Path

import pytest
from tests.wasm_linked_runner import _run_wasm_test_process, wasm_test_build_env
from tools import wasm_optimize

ROOT = Path(__file__).resolve().parents[1]
HELLO_PY = ROOT / "examples" / "hello.py"

# Size thresholds (bytes).  These are generous upper bounds; tighten as the
# compiler improves.
STANDALONE_MAX_BYTES = 15 * 1024 * 1024  # 15 MB
STANDALONE_OPT_MAX_BYTES = 10 * 1024 * 1024  # 10 MB after wasm-opt
LINKED_MAX_BYTES = 30 * 1024 * 1024  # 30 MB (includes runtime)
LINKED_OPT_MAX_BYTES = 20 * 1024 * 1024  # 20 MB after wasm-opt


def _load_wasm_link():
    path = ROOT / "tools" / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


wasm_link = _load_wasm_link()


def _molt_build_cmd() -> list[str]:
    return [sys.executable, "-m", "molt.cli", "build"]


# ---------------------------------------------------------------------------
# Tool availability tests
# ---------------------------------------------------------------------------


class TestWasmToolAvailability:
    """Verify that WASM optimization tools are detectable."""

    def test_wasm_ld_find(self) -> None:
        """wasm-ld should be discoverable via _find_wasm_ld."""
        wasm_ld = wasm_link._find_wasm_ld()
        if wasm_ld is None:
            pytest.skip(
                "wasm-ld not found; install LLVM or ensure rustup stable toolchain"
            )
        assert Path(wasm_ld).is_file()
        # Verify it actually works
        result = _run_wasm_test_process(
            [wasm_ld, "--version"],
            cwd=ROOT,
            env=os.environ,
            timeout=10,
        )
        assert result.returncode == 0
        assert "LLD" in result.stdout or "lld" in result.stdout.lower()

    def test_wasm_ld_rustup_fallback(self) -> None:
        """_find_wasm_ld should locate wasm-ld in rustup toolchains."""
        rustup_home = os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
        toolchains = Path(rustup_home) / "toolchains"
        if not toolchains.is_dir():
            pytest.skip("No rustup toolchains directory")
        import glob

        candidates = glob.glob(str(toolchains / "*/lib/rustlib/*/bin/gcc-ld/wasm-ld"))
        if not candidates:
            pytest.skip("No wasm-ld found in rustup toolchains")
        # At least one should be executable
        assert any(os.path.isfile(c) and os.access(c, os.X_OK) for c in candidates)

    def test_wasm_opt_available(self) -> None:
        """The canonical optimizer discovery should find a readable binary."""
        wasm_opt = wasm_optimize.find_wasm_opt()
        if wasm_opt is None:
            pytest.skip("wasm-opt not found")
        assert Path(wasm_opt).is_file()

    def test_wasm_tools_available(self) -> None:
        """wasm-tools should be on PATH for symbol analysis."""
        wasm_tools = shutil.which("wasm-tools")
        if wasm_tools is None:
            pytest.skip("wasm-tools not found; install via cargo")
        result = _run_wasm_test_process(
            [wasm_tools, "--version"],
            cwd=ROOT,
            env=os.environ,
            timeout=10,
        )
        assert result.returncode == 0


# ---------------------------------------------------------------------------
# Size threshold tests
# ---------------------------------------------------------------------------


def _build_wasm(
    source: Path,
    output_dir: Path,
    *,
    linked: bool = False,
) -> Path | None:
    """Build source to WASM. Returns output path or None."""
    env = wasm_test_build_env(ROOT, linked=linked)
    if linked:
        env["MOLT_WASM_LINK"] = "1"
        wasm_ld_path = wasm_link._find_wasm_ld()
        if wasm_ld_path:
            ld_dir = str(Path(wasm_ld_path).parent)
            env["PATH"] = ld_dir + os.pathsep + env.get("PATH", "")
    cmd = _molt_build_cmd() + [
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
    for name in ("output_linked.wasm", "output.wasm"):
        candidate = output_dir / name
        if candidate.exists():
            return candidate
    return None


def test_wasm_performance_build_uses_wasm_test_guard(
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
        return type("Result", (), {"returncode": 0, "stdout": "", "stderr": ""})()

    monkeypatch.setattr(sys.modules[__name__], "_run_wasm_test_process", fake_run)

    output = _build_wasm(source, out_dir, linked=False)

    assert output == out_dir / "output.wasm"
    assert captured["kwargs"]["cwd"] == ROOT
    assert captured["kwargs"]["timeout"] == 300


def _optimize_wasm_for_size(input_path: Path, output_path: Path) -> dict[str, object]:
    return wasm_optimize.optimize(
        input_path,
        output_path=output_path,
        level="Oz",
        extra_passes=("--strip-debug",),
        required_exports=wasm_optimize._collect_exports(input_path),
        timeout=120,
    )


def test_wasm_performance_size_optimizer_uses_canonical_authority(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    input_path = tmp_path / "input.wasm"
    output_path = tmp_path / "output.wasm"
    input_path.write_bytes(b"\x00asm\x01\x00\x00\x00")
    captured: dict[str, object] = {}

    def fake_optimize(path, **kwargs):  # type: ignore[no-untyped-def]
        captured["path"] = path
        captured["kwargs"] = kwargs
        return {"ok": True, "error": ""}

    monkeypatch.setattr(wasm_optimize, "optimize", fake_optimize)

    result = _optimize_wasm_for_size(input_path, output_path)

    assert result["ok"] is True
    assert captured == {
        "path": input_path,
        "kwargs": {
            "output_path": output_path,
            "level": "Oz",
            "extra_passes": ("--strip-debug",),
            "required_exports": set(),
            "timeout": 120,
        },
    }


class TestWasmSizeThresholds:
    """WASM output size must stay within defined budgets."""

    def test_standalone_size(self) -> None:
        with tempfile.TemporaryDirectory(prefix="molt-perf-") as tmpdir:
            output = _build_wasm(HELLO_PY, Path(tmpdir), linked=False)
            if output is None:
                pytest.skip("WASM build failed")
            size = output.stat().st_size
            assert size < STANDALONE_MAX_BYTES, (
                f"Standalone WASM {size:,} bytes exceeds threshold "
                f"{STANDALONE_MAX_BYTES:,} bytes"
            )

    def test_standalone_optimized_size(self) -> None:
        if wasm_optimize.find_wasm_opt() is None:
            pytest.skip("wasm-opt not available")
        with tempfile.TemporaryDirectory(prefix="molt-perf-") as tmpdir:
            tmpdir_path = Path(tmpdir)
            output = _build_wasm(HELLO_PY, tmpdir_path, linked=False)
            if output is None:
                pytest.skip("WASM build failed")
            opt_path = tmpdir_path / "optimized.wasm"
            result = _optimize_wasm_for_size(output, opt_path)
            assert result["ok"], f"wasm-opt failed: {result['error']}"
            assert opt_path.exists(), "canonical optimizer published no artifact"
            size = opt_path.stat().st_size
            assert size < STANDALONE_OPT_MAX_BYTES, (
                f"Optimized WASM {size:,} bytes exceeds threshold "
                f"{STANDALONE_OPT_MAX_BYTES:,} bytes"
            )

    def test_linked_size(self) -> None:
        if wasm_link._find_wasm_ld() is None:
            pytest.skip("wasm-ld not available")
        with tempfile.TemporaryDirectory(prefix="molt-perf-") as tmpdir:
            output = _build_wasm(HELLO_PY, Path(tmpdir), linked=True)
            if output is None:
                pytest.skip("Linked WASM build failed")
            size = output.stat().st_size
            assert size < LINKED_MAX_BYTES, (
                f"Linked WASM {size:,} bytes exceeds threshold "
                f"{LINKED_MAX_BYTES:,} bytes"
            )

    def test_linked_optimized_size(self) -> None:
        if wasm_link._find_wasm_ld() is None:
            pytest.skip("wasm-ld not available")
        if wasm_optimize.find_wasm_opt() is None:
            pytest.skip("wasm-opt not available")
        with tempfile.TemporaryDirectory(prefix="molt-perf-") as tmpdir:
            tmpdir_path = Path(tmpdir)
            output = _build_wasm(HELLO_PY, tmpdir_path, linked=True)
            if output is None:
                pytest.skip("Linked WASM build failed")
            opt_path = tmpdir_path / "linked_optimized.wasm"
            result = _optimize_wasm_for_size(output, opt_path)
            assert result["ok"], f"wasm-opt failed on linked output: {result['error']}"
            assert opt_path.exists(), "canonical optimizer published no artifact"
            size = opt_path.stat().st_size
            assert size < LINKED_OPT_MAX_BYTES, (
                f"Linked+optimized WASM {size:,} bytes exceeds threshold "
                f"{LINKED_OPT_MAX_BYTES:,} bytes"
            )


# ---------------------------------------------------------------------------
# Code section dominance test (from size audit)
# ---------------------------------------------------------------------------


class TestWasmSectionAnalysis:
    """Verify WASM section breakdown matches expectations."""

    def test_code_section_dominance(self) -> None:
        """Code section should be the largest section (88%+ per audit)."""
        with tempfile.TemporaryDirectory(prefix="molt-perf-") as tmpdir:
            output = _build_wasm(HELLO_PY, Path(tmpdir), linked=False)
            if output is None:
                pytest.skip("WASM build failed")
            data = output.read_bytes()
            sections = wasm_link._parse_sections(data)
            total = len(data)
            code_size = 0
            for sid, payload in sections:
                if sid == 10:  # Code section
                    code_size = len(payload)
            code_pct = code_size / total * 100
            assert code_pct > 50, (
                f"Code section is only {code_pct:.1f}% of total; "
                f"expected >50% (audit showed 88%)"
            )
