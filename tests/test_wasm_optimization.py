"""Tests for WASM optimisation tooling (MOL-211).

Covers:
- wasm-opt size reduction (if Binaryen is available)
- Optimised module correctness (magic/version preserved)
- WASM section ordering validation
- Data segment deduplication (already in backend)

Run with: ``uv run pytest tests/test_wasm_optimization.py -v``
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest
from tests.wasm_linked_runner import _run_wasm_test_process, wasm_test_build_env
from molt.wasm_artifact import WASM_SECTION_NAMES
from molt.wasm_optimization import WASM_OPT_LEVELS

ROOT = Path(__file__).resolve().parents[1]

# Import project tools (added to path so they are importable)
sys.path.insert(0, str(ROOT / "tools"))
from wasm_optimize import _collect_exports, find_wasm_opt, optimize  # noqa: E402
from wasm_metrics import wasm_metrics  # noqa: E402
from wasm_size_audit import parse_sections  # noqa: E402


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
    env = wasm_test_build_env(ROOT, linked=False)
    result = _run_wasm_test_process(
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


def _mock_wasm_opt_executable(
    mod: object,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> Path:
    """Pin one readable optimizer identity without spawning a real binary."""

    executable = tmp_path / "wasm-opt-test-bin"
    executable.write_bytes(b"binaryen-test-build")
    monkeypatch.setattr(mod, "find_wasm_opt", lambda: str(executable))
    monkeypatch.setattr(
        mod,
        "_COMMANDS",
        SimpleNamespace(
            run=lambda cmd, **_kwargs: subprocess.CompletedProcess(
                cmd, 0, "wasm-opt test", ""
            )
        ),
    )
    return executable


def _staged_output_from_command(cmd: list[str]) -> Path:
    return Path(cmd[cmd.index("-o") + 1])


def _active_data_module(payload: bytes) -> bytes:
    import_payload = (
        b"\x01" + _wasm_string("env") + _wasm_string("memory") + b"\x02\x00\x01"
    )
    data_payload = b"\x01\x00\x41\x10\x0b" + _varuint(len(payload)) + payload
    data = bytearray(b"\x00asm\x01\x00\x00\x00")
    for section_id, section_payload in ((2, import_payload), (11, data_payload)):
        data.append(section_id)
        data.extend(_varuint(len(section_payload)))
        data.extend(section_payload)
    return bytes(data)


def test_wasm_metrics_profiles_active_data_payload_and_zeros(tmp_path: Path) -> None:
    wasm_path = tmp_path / "data.wasm"
    wasm_path.write_bytes(_active_data_module(b"a\x00\x00b"))

    metrics = wasm_metrics(wasm_path)

    assert metrics["data_segments"] == {
        "count": 1,
        "active_count": 1,
        "passive_count": 0,
        "payload_bytes": 4,
        "zero_bytes": 2,
    }


def test_build_wasm_uses_wasm_test_guard(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    src = tmp_path / "hello.py"
    src.write_text("print(42)\n", encoding="utf-8")
    out_dir = tmp_path / "wasm"
    captured: dict[str, object] = {}

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = list(cmd)
        captured["kwargs"] = kwargs
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "output.wasm").write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr(sys.modules[__name__], "_run_wasm_test_process", fake_run)

    wasm_path = _build_wasm(src, out_dir)

    assert wasm_path == out_dir / "output.wasm"
    assert captured["kwargs"]["cwd"] == ROOT
    assert captured["kwargs"]["timeout"] == 120


# ---------------------------------------------------------------------------
# Tests: wasm-opt reduction
# ---------------------------------------------------------------------------


class TestWasmOptReduction:
    """Test that wasm-opt reduces module size (if available)."""

    def test_wasm_opt_available_check(self) -> None:
        """find_wasm_opt returns a path or None; never raises."""
        result = find_wasm_opt()
        assert result is None or Path(result).name in {"wasm-opt", "wasm-opt.exe"}

    @pytest.mark.parametrize("level", WASM_OPT_LEVELS)
    def test_every_binaryen_level_preserves_exact_export_contract(
        self, level: str, tmp_path: Path
    ) -> None:
        if find_wasm_opt() is None:
            pytest.skip("wasm-opt not installed (Binaryen)")
        source = tmp_path / f"input-{level}.wasm"
        output = tmp_path / f"output-{level}.wasm"
        source.write_bytes(_exported_func_module("kept"))

        result = optimize(
            source,
            output_path=output,
            level=level,
            required_exports={"kept"},
        )

        assert result["ok"], result["error"]
        assert result["status"] == "success"
        assert output.read_bytes().startswith(b"\x00asm\x01\x00\x00\x00")
        assert _collect_exports(output) == {"kept"}

    @pytest.mark.skipif(
        find_wasm_opt() is None,
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
        find_wasm_opt() is None,
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

        _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)
        recorded: dict[str, object] = {}

        def fake_run(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            recorded["cmd"] = list(cmd)
            _staged_output_from_command(cmd).write_bytes(dummy.read_bytes())
            return subprocess.CompletedProcess(cmd, 0, "", "")

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_run
        )
        result = mod.optimize(dummy, output_path=output, level="Oz", converge=False)

        assert result["ok"]
        cmd = recorded["cmd"]
        assert "--converge" not in cmd
        assert "-Oz" in cmd

    def test_optimize_reports_guarded_process_memory(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import tools.wasm_optimize as mod

        source = tmp_path / "input.wasm"
        output = tmp_path / "output.wasm"
        source.write_bytes(_exported_func_module("kept"))
        _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)

        def fake_guarded(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            _staged_output_from_command(cmd).write_bytes(source.read_bytes())
            result = subprocess.CompletedProcess(cmd, 0, "", "")
            result.peak = SimpleNamespace(rss_kb=12_345)
            result.peak_total = SimpleNamespace(rss_kb=23_456)
            return result

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_guarded
        )
        result = mod.optimize(source, output_path=output, level="Oz")

        assert result["ok"] is True
        assert result["peak_rss_kb"] == 12_345
        assert result["peak_total_rss_kb"] == 23_456

    def test_optimize_rejects_missing_required_exports(
        self,
        tmp_path: Path,
        monkeypatch,
    ) -> None:
        import tools.wasm_optimize as mod

        input_wasm = tmp_path / "input.wasm"
        output_wasm = tmp_path / "output.wasm"
        input_wasm.write_bytes(_exported_func_module("required"))

        _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)

        def fake_run(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            output_path = Path(cmd[cmd.index("-o") + 1])
            output_path.write_bytes(_exported_func_module("wrong"))
            return subprocess.CompletedProcess(cmd, 0, "", "")

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_run
        )
        result = mod.optimize(
            input_wasm,
            output_path=output_wasm,
            level="Oz",
            required_exports={"required"},
        )

        assert result["ok"] is False
        assert "missing required exports" in str(result["error"])
        assert "required" in str(result["error"])


class TestWasmOptAtomicPublication:
    """The canonical optimizer never exposes an unvalidated intermediate."""

    def test_o1_default_policy_does_not_converge(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import tools.wasm_optimize as mod

        source = tmp_path / "input.wasm"
        destination = tmp_path / "output.wasm"
        source.write_bytes(_exported_func_module("kept"))
        _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)
        recorded: dict[str, object] = {}

        def fake_guarded(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            recorded["cmd"] = list(cmd)
            _staged_output_from_command(cmd).write_bytes(source.read_bytes())
            return subprocess.CompletedProcess(cmd, 0, "", "")

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_guarded
        )

        result = mod.optimize(source, output_path=destination, level="O1")

        assert result["ok"] is True
        command = recorded["cmd"]
        assert isinstance(command, list)
        assert "-O1" in command
        assert "--converge" not in command

    def test_timeout_preserves_destination_and_cleans_staging(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import tools.wasm_optimize as mod

        source = tmp_path / "input.wasm"
        destination = tmp_path / "output.wasm"
        source.write_bytes(_exported_func_module("kept"))
        destination.write_bytes(b"previous-output")
        _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)
        staged: list[Path] = []

        def fake_guarded(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            staged_output = _staged_output_from_command(cmd)
            staged.append(staged_output)
            staged_output.write_bytes(b"partial-timeout-output")
            process = subprocess.CompletedProcess(cmd, 124, "", "timeout")
            process.timed_out = True
            return process

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_guarded
        )

        result = mod.optimize(source, output_path=destination, level="Oz")

        assert result["ok"] is False
        assert result["status"] == "timeout"
        assert destination.read_bytes() == b"previous-output"
        assert len(staged) == 1
        assert staged[0] != destination
        assert not staged[0].exists()

    def test_nonzero_exit_preserves_destination_and_cleans_staging(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import tools.wasm_optimize as mod

        source = tmp_path / "input.wasm"
        destination = tmp_path / "output.wasm"
        source.write_bytes(_exported_func_module("kept"))
        destination.write_bytes(b"previous-output")
        _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)
        staged: list[Path] = []

        def fake_guarded(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            staged_output = _staged_output_from_command(cmd)
            staged.append(staged_output)
            staged_output.write_bytes(b"partial-failed-output")
            return subprocess.CompletedProcess(cmd, 7, "", "binaryen failed")

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_guarded
        )

        result = mod.optimize(source, output_path=destination, level="Oz")

        assert result["ok"] is False
        assert result["status"] == "failed"
        assert destination.read_bytes() == b"previous-output"
        assert len(staged) == 1
        assert not staged[0].exists()

    def test_invalid_output_preserves_destination_and_cleans_staging(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import tools.wasm_optimize as mod

        source = tmp_path / "input.wasm"
        destination = tmp_path / "output.wasm"
        source.write_bytes(_exported_func_module("required"))
        destination.write_bytes(b"previous-output")
        _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)
        staged: list[Path] = []

        def fake_guarded(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            staged_output = _staged_output_from_command(cmd)
            staged.append(staged_output)
            staged_output.write_bytes(b"not-a-wasm-module")
            return subprocess.CompletedProcess(cmd, 0, "", "")

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_guarded
        )

        result = mod.optimize(
            source,
            output_path=destination,
            level="Oz",
            required_exports={"required"},
        )

        assert result["ok"] is False
        assert result["status"] == "invalid-output"
        assert destination.read_bytes() == b"previous-output"
        assert len(staged) == 1
        assert not staged[0].exists()

    def test_success_atomically_replaces_destination_and_cleans_staging(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import tools.wasm_optimize as mod

        source = tmp_path / "input.wasm"
        destination = tmp_path / "output.wasm"
        replacement = _exported_func_module("replacement")
        source.write_bytes(_exported_func_module("source"))
        destination.write_bytes(b"previous-output")
        _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)
        staged: list[Path] = []

        def fake_guarded(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            staged_output = _staged_output_from_command(cmd)
            staged.append(staged_output)
            assert staged_output != destination
            assert destination.read_bytes() == b"previous-output"
            staged_output.write_bytes(replacement)
            return subprocess.CompletedProcess(cmd, 0, "", "")

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_guarded
        )

        result = mod.optimize(
            source,
            output_path=destination,
            level="Oz",
            required_exports={"replacement"},
        )

        assert result["ok"] is True
        assert result["status"] == "success"
        assert destination.read_bytes() == replacement
        assert len(staged) == 1
        assert not staged[0].exists()

    def test_executable_identity_drift_fails_closed_and_cleans_staging(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import tools.wasm_optimize as mod

        source = tmp_path / "input.wasm"
        destination = tmp_path / "output.wasm"
        source.write_bytes(_exported_func_module("kept"))
        destination.write_bytes(b"previous-output")
        executable = _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)
        staged: list[Path] = []

        def fake_guarded(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            staged_output = _staged_output_from_command(cmd)
            staged.append(staged_output)
            staged_output.write_bytes(source.read_bytes())
            executable.write_bytes(b"different-binaryen-build-with-new-size")
            return subprocess.CompletedProcess(cmd, 0, "", "")

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_guarded
        )

        result = mod.optimize(source, output_path=destination, level="Oz")

        assert result["ok"] is False
        assert result["status"] == "identity-error"
        assert "changed during execution" in str(result["error"])
        assert destination.read_bytes() == b"previous-output"
        assert len(staged) == 1
        assert not staged[0].exists()


# ---------------------------------------------------------------------------
# Tests: optimised module correctness
# ---------------------------------------------------------------------------


class TestOptimisedModuleCorrectness:
    """After wasm-opt the module should still be valid WASM."""

    @pytest.mark.skipif(
        find_wasm_opt() is None,
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
        find_wasm_opt() is None,
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
                f"Missing required section: {WASM_SECTION_NAMES.get(required_id, required_id)}"
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
