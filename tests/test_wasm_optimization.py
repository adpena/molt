"""Tests for WASM optimisation tooling (MOL-211).

Covers:
- wasm-opt size reduction (if Binaryen is available)
- Optimised module correctness (magic/version preserved)
- WASM section ordering validation
- Data segment deduplication (already in backend)

Run with: ``uv run pytest tests/test_wasm_optimization.py -v``
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]

# Import project tools (added to path so they are importable)
sys.path.insert(0, str(ROOT / "tools"))
from wasm_optimize import find_wasm_opt, optimize  # noqa: E402
from wasm_size_audit import parse_sections, SECTION_NAMES  # noqa: E402


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _skip_unless_wasm() -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo not found — cannot build WASM target")


def _molt_build_cmd() -> list[str]:
    return [sys.executable, "-m", "molt.cli", "build"]


def _build_wasm(src: Path, out_dir: Path) -> Path:
    """Compile *src* to an unlinked WASM module, return path."""
    out_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    repo_src = str(ROOT / "src")
    current_pythonpath = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = (
        repo_src + os.pathsep + current_pythonpath if current_pythonpath else repo_src
    )
    env["MOLT_WASM_LINKED"] = "0"
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_MIDEND_DISABLE", "1")
    result = subprocess.run(
        _molt_build_cmd()
        + [
            str(src),
            "--target",
            "wasm",
            "--emit",
            "wasm",
            "--out-dir",
            str(out_dir),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env=env,
        timeout=120,
    )
    assert result.returncode == 0, f"WASM build failed:\n{result.stderr}"
    wasm_path = out_dir / "output.wasm"
    assert wasm_path.exists(), "output.wasm not produced"
    return wasm_path


def _varuint(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def _wasm_string(value: str) -> bytes:
    raw = value.encode("utf-8")
    return _varuint(len(raw)) + raw


def _exported_func_module(export_name: str) -> bytes:
    sections: list[tuple[int, bytes]] = []
    sections.append((1, b"\x01\x60\x00\x00"))
    sections.append((3, b"\x01\x00"))
    export_payload = b"\x01" + _wasm_string(export_name) + b"\x00\x00"
    sections.append((7, export_payload))
    sections.append((10, b"\x01\x02\x00\x0b"))
    data = bytearray(b"\x00asm\x01\x00\x00\x00")
    for section_id, payload in sections:
        data.append(section_id)
        data.extend(_varuint(len(payload)))
        data.extend(payload)
    return bytes(data)


# ---------------------------------------------------------------------------
# Tests: wasm-opt reduction
# ---------------------------------------------------------------------------


class TestWasmOptReduction:
    """Test that wasm-opt reduces module size (if available)."""

    def test_wasm_opt_available_check(self) -> None:
        """find_wasm_opt returns a path or None; never raises."""
        result = find_wasm_opt()
        assert result is None or Path(result).name == "wasm-opt"

    @pytest.mark.skipif(
        shutil.which("wasm-opt") is None,
        reason="wasm-opt not installed (Binaryen)",
    )
    def test_optimize_reduces_size(self, tmp_path: Path) -> None:
        """wasm-opt -O2 should reduce the module size."""
        _skip_unless_wasm()
        src = ROOT / "examples" / "hello.py"
        wasm_path = _build_wasm(src, tmp_path / "wasm")
        original_size = wasm_path.stat().st_size

        result = optimize(wasm_path, output_path=tmp_path / "optimized.wasm")
        assert result["ok"], f"wasm-opt failed: {result['error']}"
        assert result["output_bytes"] > 0
        assert result["output_bytes"] < original_size, (
            f"Expected size reduction: {original_size} -> {result['output_bytes']}"
        )
        assert result["reduction_pct"] > 0

    @pytest.mark.skipif(
        shutil.which("wasm-opt") is None,
        reason="wasm-opt not installed (Binaryen)",
    )
    def test_optimize_oz_reduces_more_than_o1(self, tmp_path: Path) -> None:
        """Oz (size-focused) should yield smaller output than O1."""
        _skip_unless_wasm()
        src = ROOT / "examples" / "hello.py"
        wasm_path = _build_wasm(src, tmp_path / "wasm")

        r_o1 = optimize(wasm_path, output_path=tmp_path / "o1.wasm", level="O1")
        r_oz = optimize(wasm_path, output_path=tmp_path / "oz.wasm", level="Oz")
        assert r_o1["ok"] and r_oz["ok"]
        # Oz should be at most as large as O1 (usually smaller)
        assert r_oz["output_bytes"] <= r_o1["output_bytes"] * 1.01  # 1% tolerance

    def test_optimize_missing_wasm_opt(self, tmp_path: Path) -> None:
        """Graceful failure when wasm-opt is not found."""
        # Create a dummy .wasm file
        dummy = tmp_path / "dummy.wasm"
        dummy.write_bytes(b"\x00asm\x01\x00\x00\x00")
        # Temporarily hide wasm-opt by testing the logic path
        import tools.wasm_optimize as mod

        orig = mod.find_wasm_opt
        mod.find_wasm_opt = lambda: None
        try:
            result = mod.optimize(dummy)
            assert not result["ok"]
            assert "not found" in result["error"]
        finally:
            mod.find_wasm_opt = orig

    def test_optimize_invalid_level(self, tmp_path: Path) -> None:
        """Invalid optimisation level returns an error, not a crash."""
        dummy = tmp_path / "dummy.wasm"
        dummy.write_bytes(b"\x00asm\x01\x00\x00\x00")
        result = optimize(dummy, level="O99")  # type: ignore[arg-type]
        assert not result["ok"]
        assert "Invalid" in str(result["error"])

    def test_optimize_can_disable_converge_flag(
        self, tmp_path: Path, monkeypatch
    ) -> None:
        dummy = tmp_path / "dummy.wasm"
        dummy.write_bytes(b"\x00asm\x01\x00\x00\x00")
        output = tmp_path / "out.wasm"

        import tools.wasm_optimize as mod

        monkeypatch.setattr(mod, "find_wasm_opt", lambda: "/usr/bin/wasm-opt")
        recorded: dict[str, object] = {}

        def fake_run(cmd, capture_output, text, timeout):  # type: ignore[no-untyped-def]
            recorded["cmd"] = list(cmd)
            output.write_bytes(dummy.read_bytes())
            return subprocess.CompletedProcess(cmd, 0, "", "")

        monkeypatch.setattr(mod.subprocess, "run", fake_run)
        result = mod.optimize(dummy, output_path=output, level="Oz", converge=False)

        assert result["ok"]
        cmd = recorded["cmd"]
        assert "--converge" not in cmd
        assert "-Oz" in cmd

    def test_optimize_rejects_missing_required_exports(
        self,
        tmp_path: Path,
        monkeypatch,
    ) -> None:
        import tools.wasm_optimize as mod

        input_wasm = tmp_path / "input.wasm"
        output_wasm = tmp_path / "output.wasm"
        input_wasm.write_bytes(_exported_func_module("required"))

        monkeypatch.setattr(mod, "find_wasm_opt", lambda: "/usr/bin/wasm-opt")

        def fake_run(cmd, capture_output, text, timeout):  # type: ignore[no-untyped-def]
            del capture_output, text, timeout
            output_path = Path(cmd[cmd.index("-o") + 1])
            output_path.write_bytes(_exported_func_module("wrong"))
            return subprocess.CompletedProcess(cmd, 0, "", "")

        monkeypatch.setattr(mod.subprocess, "run", fake_run)

        result = mod.optimize(
            input_wasm,
            output_path=output_wasm,
            level="Oz",
            required_exports={"required"},
        )

        assert result["ok"] is False
        assert "missing required exports" in str(result["error"])
        assert "required" in str(result["error"])


# ---------------------------------------------------------------------------
# Tests: optimised module correctness
# ---------------------------------------------------------------------------


class TestOptimisedModuleCorrectness:
    """After wasm-opt the module should still be valid WASM."""

    @pytest.mark.skipif(
        shutil.which("wasm-opt") is None,
        reason="wasm-opt not installed (Binaryen)",
    )
    def test_optimised_module_has_wasm_header(self, tmp_path: Path) -> None:
        _skip_unless_wasm()
        src = ROOT / "examples" / "hello.py"
        wasm_path = _build_wasm(src, tmp_path / "wasm")
        opt_path = tmp_path / "optimized.wasm"
        result = optimize(wasm_path, output_path=opt_path)
        assert result["ok"]

        data = opt_path.read_bytes()
        assert data[:4] == b"\x00asm", "Missing WASM magic bytes after optimisation"
        assert data[4:8] == b"\x01\x00\x00\x00", (
            "Unexpected WASM version after optimisation"
        )

    @pytest.mark.skipif(
        shutil.which("wasm-opt") is None,
        reason="wasm-opt not installed (Binaryen)",
    )
    def test_optimised_module_has_code_section(self, tmp_path: Path) -> None:
        _skip_unless_wasm()
        src = ROOT / "examples" / "hello.py"
        wasm_path = _build_wasm(src, tmp_path / "wasm")
        opt_path = tmp_path / "optimized.wasm"
        result = optimize(wasm_path, output_path=opt_path)
        assert result["ok"]

        sections = parse_sections(opt_path)
        code_sections = [s for s in sections if s.name == "code"]
        assert len(code_sections) >= 1, "Optimised module has no code section"
        assert code_sections[0].size > 0, "Code section is empty after optimisation"


# ---------------------------------------------------------------------------
# Tests: WASM section ordering
# ---------------------------------------------------------------------------


class TestWasmSectionOrdering:
    """WASM spec requires sections in ascending ID order (custom can appear anywhere)."""

    def test_section_order_is_valid(self, tmp_path: Path) -> None:
        _skip_unless_wasm()
        src = ROOT / "examples" / "hello.py"
        wasm_path = _build_wasm(src, tmp_path / "wasm")
        sections = parse_sections(wasm_path)

        # Non-custom sections must appear in ascending ID order.
        non_custom_ids = [s.id for s in sections if s.id != 0]
        for i in range(1, len(non_custom_ids)):
            assert non_custom_ids[i] >= non_custom_ids[i - 1], (
                f"Section ordering violation: section {non_custom_ids[i]} "
                f"appears after section {non_custom_ids[i - 1]}"
            )

    def test_required_sections_present(self, tmp_path: Path) -> None:
        """A Molt WASM module should have at least type, function, code sections."""
        _skip_unless_wasm()
        src = ROOT / "examples" / "hello.py"
        wasm_path = _build_wasm(src, tmp_path / "wasm")
        sections = parse_sections(wasm_path)
        section_ids = {s.id for s in sections}

        # type=1, function=3, code=10 are required for any non-trivial module
        for required_id in (1, 3, 10):
            assert required_id in section_ids, (
                f"Missing required section: {SECTION_NAMES.get(required_id, required_id)}"
            )


# ---------------------------------------------------------------------------
# Tests: data segment deduplication
# ---------------------------------------------------------------------------


class TestDataSegmentDedup:
    """Verify that duplicate data segments are deduplicated by the backend."""

    def test_data_section_not_bloated(self, tmp_path: Path) -> None:
        """Data section should be a reasonable fraction of the module."""
        _skip_unless_wasm()
        src = ROOT / "examples" / "hello.py"
        wasm_path = _build_wasm(src, tmp_path / "wasm")
        total = wasm_path.stat().st_size
        sections = parse_sections(wasm_path)
        data_size = sum(s.size for s in sections if s.name == "data")

        # Data section should not exceed 40% of total (would indicate dup bloat)
        ratio = data_size / total if total > 0 else 0
        assert ratio < 0.40, (
            f"Data section is {data_size:,} bytes ({ratio * 100:.1f}% of {total:,}) — "
            "possible deduplication failure"
        )

    def test_two_similar_programs_share_runtime_data(self, tmp_path: Path) -> None:
        """Two programs should have nearly identical data section sizes
        (runtime dominates, user data is small)."""
        _skip_unless_wasm()
        src_a = ROOT / "examples" / "hello.py"
        src_b = ROOT / "examples" / "simple_ret.py"
        wasm_a = _build_wasm(src_a, tmp_path / "wasm_a")
        wasm_b = _build_wasm(src_b, tmp_path / "wasm_b")

        secs_a = parse_sections(wasm_a)
        secs_b = parse_sections(wasm_b)
        data_a = sum(s.size for s in secs_a if s.name == "data")
        data_b = sum(s.size for s in secs_b if s.name == "data")

        if data_a == 0 and data_b == 0:
            pytest.skip("No data sections in either module")

        # Data sections should be within 10% of each other (shared runtime)
        larger = max(data_a, data_b)
        smaller = min(data_a, data_b)
        ratio = smaller / larger if larger > 0 else 1.0
        assert ratio > 0.80, (
            f"Data section sizes differ too much: {data_a:,} vs {data_b:,} "
            f"(ratio {ratio:.2f}) — possible dedup issue"
        )
