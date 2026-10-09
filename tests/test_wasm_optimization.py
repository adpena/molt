"""Tests for WASM optimisation tooling (MOL-211).

Covers:
- wasm-opt size reduction (if Binaryen is available)
- Optimised module correctness (magic/version preserved)
- WASM section ordering validation
- Data segment deduplication (already in backend)

Run with: ``uv run pytest tests/test_wasm_optimization.py -v``
"""

from __future__ import annotations

import hashlib
import shutil
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.toolchain_identity import stable_regular_file_identity
from molt.wasm_artifact import WASM_SECTION_NAMES
from molt.wasm_optimization import WASM_OPT_LEVELS, wasm_opt_pipeline
from molt.wasm_optimizer_identity import (
    WASM_OPTIMIZER_ATTESTATION_SCHEMA,
    WasmOptimizerExecutableIdentity,
    WasmOptimizerIdentityError,
    build_wasm_optimizer_attestation,
    encode_wasm_optimizer_attestation,
    load_wasm_optimizer_attestation,
    validate_wasm_optimizer_attestation,
    wasm_optimizer_pipeline_authority_sha256,
)
from tests.wasm_linked_runner import _run_wasm_test_process, wasm_test_build_env
from tests.process_guard_common import install_module_view

ROOT = Path(__file__).resolve().parents[1]

# Import project tools (added to path so they are importable)
sys.path.insert(0, str(ROOT / "tools"))
from wasm_link_edit import _standard_section_order_error  # noqa: E402
from wasm_metrics import wasm_metrics  # noqa: E402
from wasm_optimize import _export_names, find_wasm_opt, optimize  # noqa: E402
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
    # The compiler source tree is never artifact custody. Keep the runtime
    # generation beside pytest's externally rooted outputs; module-scoped
    # fixtures share this directory without publishing into ``ROOT / wasm``.
    env["MOLT_WASM_RUNTIME_DIR"] = str(out_dir.parent / "runtime")
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
    )
    assert result.returncode == 0, f"WASM build failed:\n{result.stderr}"
    wasm_path = out_dir / "output.wasm"
    assert wasm_path.exists(), "output.wasm not produced"
    return wasm_path


@pytest.fixture(scope="module")
def hello_wasm_path(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Build the shared read-only hello module once for this test family."""

    _skip_unless_wasm()
    return _build_wasm(
        ROOT / "examples" / "hello.py",
        tmp_path_factory.mktemp("wasm-optimization-hello"),
    )


@pytest.fixture(scope="module")
def simple_ret_wasm_path(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Build the shared read-only comparison module once for this test family."""

    _skip_unless_wasm()
    return _build_wasm(
        ROOT / "examples" / "simple_ret.py",
        tmp_path_factory.mktemp("wasm-optimization-simple-ret"),
    )


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
    stable = stable_regular_file_identity(executable, label="test wasm-opt")
    monkeypatch.setattr(
        mod,
        "wasm_optimizer_executable_identity",
        lambda _selected: WasmOptimizerExecutableIdentity(
            executable=stable,
            binaryen_version="wasm-opt version 130 (version_130)",
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
    assert "timeout" not in captured["kwargs"]
    captured_env = captured["kwargs"]["env"]
    assert Path(captured_env["MOLT_WASM_RUNTIME_DIR"]) == tmp_path / "runtime"


# ---------------------------------------------------------------------------
# Tests: wasm-opt reduction
# ---------------------------------------------------------------------------


class TestWasmOptReduction:
    """Test that wasm-opt reduces module size (if available)."""

    def test_wasm_opt_available_check(self) -> None:
        """find_wasm_opt returns a path or None; never raises."""
        result = find_wasm_opt()
        assert result is None or Path(result).name in {"wasm-opt", "wasm-opt.exe"}

    def test_invalid_explicit_pin_never_falls_through_to_hostile_path(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import molt.wasm_optimizer_identity as identity

        hostile = tmp_path / ("wasm-opt.exe" if sys.platform == "win32" else "wasm-opt")
        hostile.write_bytes(b"hostile ambient optimizer")
        monkeypatch.setenv("MOLT_WASM_OPT", str(tmp_path / "missing-explicit-pin"))
        monkeypatch.setattr(identity.shutil, "which", lambda _name: str(hostile))

        assert find_wasm_opt() is None

    def test_managed_discovery_selects_only_the_manifest_owned_release(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import molt.wasm_optimizer_identity as identity

        executable_name = "wasm-opt.exe" if sys.platform == "win32" else "wasm-opt"
        obsolete = (
            tmp_path / "toolchains" / "binaryen-version_9" / "bin" / executable_name
        )
        expected = (
            tmp_path / "toolchains" / "binaryen-version_130" / "bin" / executable_name
        )
        obsolete.parent.mkdir(parents=True)
        expected.parent.mkdir(parents=True)
        obsolete.write_bytes(b"obsolete-binaryen")
        expected.write_bytes(b"manifest-owned-binaryen")
        monkeypatch.delenv("MOLT_WASM_OPT", raising=False)
        monkeypatch.setenv("MOLT_TARGET_ROOT", str(tmp_path))
        install_module_view(
            monkeypatch, "shutil", shutil, identity, which=lambda _name: None
        )
        asset = SimpleNamespace(
            archive_root="binaryen-version_130",
            executable=f"bin/{executable_name}",
        )
        monkeypatch.setattr(identity, "binaryen_host_asset", lambda _root: asset)
        monkeypatch.setattr(
            identity,
            "_managed_binaryen_asset",
            lambda path: asset if path == expected.resolve() else None,
        )

        assert identity.find_wasm_opt() == str(expected)

        expected.unlink()
        assert identity.find_wasm_opt() is None

    def test_managed_discovery_rejects_missing_custody_receipt(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import molt.wasm_optimizer_identity as identity

        executable_name = "wasm-opt.exe" if sys.platform == "win32" else "wasm-opt"
        executable = (
            tmp_path / "toolchains" / "binaryen-version_130" / "bin" / executable_name
        )
        executable.parent.mkdir(parents=True)
        executable.write_bytes(b"unreceipted-binaryen")
        asset = SimpleNamespace(
            archive_root="binaryen-version_130",
            executable=f"bin/{executable_name}",
        )
        monkeypatch.delenv("MOLT_WASM_OPT", raising=False)
        monkeypatch.setenv("MOLT_TARGET_ROOT", str(tmp_path))
        install_module_view(
            monkeypatch, "shutil", shutil, identity, which=lambda _name: None
        )
        monkeypatch.setattr(identity, "binaryen_host_asset", lambda _root: asset)

        assert identity.find_wasm_opt() is None

    def test_optimizer_attestation_binds_tool_pipeline_and_published_bytes(
        self, tmp_path: Path
    ) -> None:
        executable = (tmp_path / "wasm-opt").resolve()
        pipeline = list(
            wasm_opt_pipeline(
                "O1",
                extra_passes=("--vacuum",),
                converge=False,
                apply_level=True,
            )
        )
        input_sha256 = hashlib.sha256(b"input").hexdigest()
        output_sha256 = hashlib.sha256(b"optimizer-output").hexdigest()
        payload = build_wasm_optimizer_attestation(
            {
                "ok": True,
                "binaryen_version": "wasm-opt version 130 (version_130)",
                "wasm_opt_path": str(executable),
                "wasm_opt_sha256": "1" * 64,
                "optimization_level": "O1",
                "optimization_converge": False,
                "optimization_apply_level": True,
                "optimization_preserve_debug": False,
                "optimization_extra_passes": ["--vacuum"],
                "pipeline": pipeline,
                "optimizer_input_sha256": input_sha256,
                "optimizer_output_sha256": output_sha256,
                "cache_hit": False,
                "wasm_opt_wall_ms": 123.5,
                "wasm_opt_peak_rss_kb": 4096,
            },
            published_output=b"published",
        )

        assert payload == {
            "schema": WASM_OPTIMIZER_ATTESTATION_SCHEMA,
            "status": "success",
            "binaryen_version": "wasm-opt version 130 (version_130)",
            "wasm_opt_sha256": "1" * 64,
            "optimization_level": "O1",
            "optimization_converge": False,
            "optimization_apply_level": True,
            "optimization_preserve_debug": False,
            "optimization_extra_passes": ["--vacuum"],
            "pipeline": pipeline,
            "pipeline_authority_sha256": wasm_optimizer_pipeline_authority_sha256(
                level="O1",
                converge=False,
                apply_level=True,
                preserve_debug=False,
                extra_passes=("--vacuum",),
                pipeline=pipeline,
            ),
            "optimizer_input_sha256": input_sha256,
            "optimizer_output_sha256": output_sha256,
            "published_output_sha256": hashlib.sha256(b"published").hexdigest(),
        }
        warm_payload = build_wasm_optimizer_attestation(
            {
                **payload,
                "wasm_opt_path": "/different/host/wasm-opt",
                "cache_hit": True,
                "wasm_opt_wall_ms": 0.0,
                "wasm_opt_peak_rss_kb": 1,
            },
            published_output=b"published",
        )
        assert warm_payload == payload
        assert "wasm_opt_path" not in encode_wasm_optimizer_attestation(payload)
        assert "cache_hit" not in encode_wasm_optimizer_attestation(payload)

    @pytest.mark.parametrize(
        ("wire", "message"),
        (
            ('{"schema":"first","schema":"second"}', "duplicate JSON key"),
            ('{"schema":-Infinity}', "non-finite JSON number"),
        ),
    )
    def test_optimizer_attestation_loader_rejects_inexact_json(
        self, tmp_path: Path, wire: str, message: str
    ) -> None:
        path = tmp_path / "output.wasm.wasm-opt.json"
        path.write_text(wire, encoding="utf-8")

        with pytest.raises(WasmOptimizerIdentityError, match=message):
            load_wasm_optimizer_attestation(path)

    @pytest.mark.parametrize(
        "version",
        (
            "unknown",
            "wasm-opt version 130",
            "wasm-opt version 130 (version_131)",
            "wasm-opt version 130 (version_130)\n",
            "wasm-opt version 130 (version_130) trailing",
        ),
    )
    def test_optimizer_attestation_rejects_false_version_evidence(
        self, tmp_path: Path, version: str
    ) -> None:
        pipeline = list(wasm_opt_pipeline("O1", converge=False))
        payload = {
            "schema": WASM_OPTIMIZER_ATTESTATION_SCHEMA,
            "status": "success",
            "binaryen_version": version,
            "wasm_opt_sha256": "1" * 64,
            "optimization_level": "O1",
            "optimization_converge": False,
            "optimization_apply_level": True,
            "optimization_preserve_debug": False,
            "optimization_extra_passes": [],
            "pipeline": pipeline,
            "pipeline_authority_sha256": wasm_optimizer_pipeline_authority_sha256(
                level="O1",
                converge=False,
                apply_level=True,
                preserve_debug=False,
                extra_passes=(),
                pipeline=pipeline,
            ),
            "optimizer_input_sha256": "2" * 64,
            "optimizer_output_sha256": "3" * 64,
            "published_output_sha256": "2" * 64,
        }

        with pytest.raises(WasmOptimizerIdentityError, match="attestation is invalid"):
            validate_wasm_optimizer_attestation(payload)

    @pytest.mark.parametrize(
        ("returncode", "stdout", "stderr"),
        (
            (1, "wasm-opt version 130 (version_130)\n", ""),
            (0, "wasm-opt version 130 (version_130)\n", "warning"),
            (0, "unknown\n", ""),
        ),
    )
    def test_optimizer_rejects_invalid_version_probe_before_execution(
        self,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
        returncode: int,
        stdout: str,
        stderr: str,
    ) -> None:
        import molt.binaryen_identity as binaryen_identity
        import tools.wasm_optimize as mod

        source = tmp_path / "input.wasm"
        source.write_bytes(_exported_func_module("kept"))
        executable = tmp_path / "wasm-opt"
        executable.write_bytes(b"binaryen-test-build")
        monkeypatch.setattr(mod, "find_wasm_opt", lambda: str(executable))
        install_module_view(
            monkeypatch,
            "subprocess",
            subprocess,
            binaryen_identity,
            run=lambda cmd, **_kwargs: subprocess.CompletedProcess(
                cmd, returncode, stdout, stderr
            ),
        )
        invoked = False

        def reject_execution(*_args, **_kwargs):  # type: ignore[no-untyped-def]
            nonlocal invoked
            invoked = True
            raise AssertionError("optimizer execution must not start")

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", reject_execution
        )

        result = mod.optimize(source, output_path=tmp_path / "output.wasm")

        assert result["ok"] is False
        assert result["status"] == "identity-error"
        assert "version identity is invalid" in str(result["error"])
        assert invoked is False

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
        assert _export_names(output) == {"kept"}

    @pytest.mark.skipif(
        find_wasm_opt() is None,
        reason="wasm-opt not installed (Binaryen)",
    )
    def test_optimize_reduces_size(self, hello_wasm_path: Path, tmp_path: Path) -> None:
        """wasm-opt -O2 should reduce the module size."""
        original_size = hello_wasm_path.stat().st_size

        result = optimize(hello_wasm_path, output_path=tmp_path / "optimized.wasm")
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
    def test_optimize_oz_reduces_more_than_o1(
        self, hello_wasm_path: Path, tmp_path: Path
    ) -> None:
        """Oz (size-focused) should yield smaller output than O1."""
        r_o1 = optimize(hello_wasm_path, output_path=tmp_path / "o1.wasm", level="O1")
        r_oz = optimize(hello_wasm_path, output_path=tmp_path / "oz.wasm", level="Oz")
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

    def test_optimizer_staging_name_is_bounded_for_long_final_name(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import tools.wasm_optimize as mod
        from molt.file_publication import is_owned_staged_file_path

        source = tmp_path / "input.wasm"
        destination = tmp_path / (("nested-atomic-output-" * 11) + ".wasm")
        source.write_bytes(_exported_func_module("kept"))
        _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)
        staged: list[Path] = []

        def fake_guarded(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            staged_output = _staged_output_from_command(cmd)
            staged.append(staged_output)
            process = subprocess.CompletedProcess(cmd, 124, "", "timeout")
            process.timed_out = True
            return process

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_guarded
        )

        result = mod.optimize(source, output_path=destination, level="Oz")

        assert result["status"] == "timeout"
        assert len(staged) == 1
        assert staged[0].parent == destination.parent
        assert is_owned_staged_file_path(staged[0], destination)
        assert staged[0].suffix == ".wasm"
        assert len(staged[0].name) < 100
        assert destination.name not in staged[0].name
        assert not staged[0].exists()

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
        replacement = _exported_func_module("source")
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
            required_exports={"source"},
        )

        assert result["ok"] is True
        assert result["status"] == "success"
        assert destination.read_bytes() == replacement
        assert len(staged) == 1
        assert not staged[0].exists()

    def test_implicit_contract_preserves_every_input_export(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import tools.wasm_optimize as mod

        source = tmp_path / "input.wasm"
        destination = tmp_path / "output.wasm"
        source.write_bytes(_exported_func_module("required"))
        destination.write_bytes(b"previous-output")
        _mock_wasm_opt_executable(mod, tmp_path, monkeypatch)

        def fake_guarded(cmd, **_kwargs):  # type: ignore[no-untyped-def]
            _staged_output_from_command(cmd).write_bytes(_exported_func_module("wrong"))
            return subprocess.CompletedProcess(cmd, 0, "", "")

        monkeypatch.setattr(
            mod.harness_memory_guard, "guarded_completed_process", fake_guarded
        )

        result = mod.optimize(source, output_path=destination, level="Oz")

        assert result["ok"] is False
        assert result["status"] == "invalid-output"
        assert "missing required exports: required" in str(result["error"])
        assert destination.read_bytes() == b"previous-output"

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
    def test_optimised_module_has_wasm_header(
        self, hello_wasm_path: Path, tmp_path: Path
    ) -> None:
        opt_path = tmp_path / "optimized.wasm"
        result = optimize(hello_wasm_path, output_path=opt_path)
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
    def test_optimised_module_has_code_section(
        self, hello_wasm_path: Path, tmp_path: Path
    ) -> None:
        opt_path = tmp_path / "optimized.wasm"
        result = optimize(hello_wasm_path, output_path=opt_path)
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

    def test_section_order_is_canonical(self, hello_wasm_path: Path) -> None:
        error = _standard_section_order_error(hello_wasm_path.read_bytes())

        assert error is None, error

    def test_required_sections_present(self, hello_wasm_path: Path) -> None:
        """A Molt WASM module should have at least type, function, code sections."""
        sections = parse_sections(hello_wasm_path)
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

    def test_data_section_not_bloated(self, hello_wasm_path: Path) -> None:
        """Data section should be a reasonable fraction of the module."""
        total = hello_wasm_path.stat().st_size
        sections = parse_sections(hello_wasm_path)
        data_size = sum(s.size for s in sections if s.name == "data")

        # Data section should not exceed 40% of total (would indicate dup bloat)
        ratio = data_size / total if total > 0 else 0
        assert ratio < 0.40, (
            f"Data section is {data_size:,} bytes ({ratio * 100:.1f}% of {total:,}) — "
            "possible deduplication failure"
        )

    def test_two_similar_programs_share_runtime_data(
        self, hello_wasm_path: Path, simple_ret_wasm_path: Path
    ) -> None:
        """Two programs should have nearly identical data section sizes
        (runtime dominates, user data is small)."""
        secs_a = parse_sections(hello_wasm_path)
        secs_b = parse_sections(simple_ret_wasm_path)
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
