"""Fast contract tests; all compiler and runtime execution is substituted."""

from __future__ import annotations

import importlib.util
import subprocess
import tomllib
from pathlib import Path
from types import ModuleType, SimpleNamespace

import pytest

from molt.target_python import (
    SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS,
    TargetPythonVersion,
)
from tests import test_finally_pending_observer_parity as parity
from tools.compat.comparison import Outputs


def _version(short: str) -> TargetPythonVersion:
    major, minor = map(int, short.split("."))
    return TargetPythonVersion(major, minor, 0)


def test_oracle_isolates_native_compiler_inputs_without_losing_guard_custody(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    env = {
        "PATH": "native-tools",
        "MOLT_MEMORY_LIMIT_GB": "2",
        "MOLT_SESSION_ID": "owned-oracle",
        "PYTHONPATH": "molt-stdlib",
        "CC": "wasm-cc",
        "CFLAGS": "-Imolt/include",
        "CPATH": "molt/include",
        "Include": "molt/include",
        "MSSdk": "1",
        "_PYTHON_SYSCONFIGDATA_NAME": "cross-config",
    }
    calls: list[list[str]] = []

    def run(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append(argv)
        assert kwargs["env"] == {
            "PATH": "native-tools",
            "MOLT_MEMORY_LIMIT_GB": "2",
            "MOLT_SESSION_ID": "owned-oracle",
        }
        assert argv[:2] == [parity.sys.executable, "-I"]
        return subprocess.CompletedProcess(argv, 0, "oracle-output\n", "")

    monkeypatch.setattr(parity, "run_native_test_process", run)
    result = parity._run_cpython_oracle(tmp_path, env)
    assert result == Outputs("oracle-output\n", "", 0)
    assert calls[0][2] == str(parity.FIXTURE / "build_cpython.py")
    assert calls[1][-2:] == [str(tmp_path / "cpython" / "lib"), str(parity.PROGRAM)]
    assert env["CC"] == "wasm-cc", "oracle isolation must not mutate Molt's environment"


@pytest.mark.parametrize("stage", ["build", "run"])
def test_oracle_failure_stops_before_accepting_outputs(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, stage: str
) -> None:
    calls = 0

    def run(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        nonlocal calls
        calls += 1
        failed = calls == (1 if stage == "build" else 2)
        return subprocess.CompletedProcess(
            argv, 7 if failed else 0, "", "oracle failed"
        )

    monkeypatch.setattr(parity, "run_native_test_process", run)
    with pytest.raises(AssertionError, match="oracle failed"):
        parity._run_cpython_oracle(tmp_path, {})
    assert calls == (1 if stage == "build" else 2)


@pytest.mark.parametrize("short", SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS)
@pytest.mark.parametrize("target", ["native", "wasm"])
def test_extension_command_forwards_exact_oracle_version(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, short: str, target: str
) -> None:
    class Submitted(RuntimeError):
        pass

    def run(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        assert argv[argv.index("--target") + 1] == target
        assert argv[argv.index("--python-version") + 1] == short
        raise Submitted

    monkeypatch.setattr(parity, "run_native_test_process", run)
    with pytest.raises(Submitted):
        parity._build_extension(
            tmp_path, {}, target=target, target_python=_version(short)
        )


def test_unverified_interpreter_is_rejected_before_toolchain_or_build(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    def reject(target: TargetPythonVersion) -> TargetPythonVersion:
        assert target.feature_version == parity.sys.version_info[:2]
        raise ValueError("unverified interpreter tuple")

    def unexpected() -> None:
        pytest.fail("toolchain setup must not run for a rejected interpreter")

    monkeypatch.setattr(parity, "require_verified_subset_target", reject)
    monkeypatch.setattr(parity, "require_wasm_toolchain", unexpected)
    with pytest.raises(ValueError, match="unverified interpreter tuple"):
        parity.test_finally_pending_observer_native_wasm_parity(tmp_path, monkeypatch)


@pytest.mark.parametrize("short", SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS)
@pytest.mark.parametrize("divergence", ["none", "native", "wasm", "both"])
def test_parity_forwards_version_and_rejects_shared_backend_divergence(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, short: str, divergence: str
) -> None:
    target = _version(short)
    extension_targets: list[tuple[str, TargetPythonVersion]] = []
    actual_native = "wrong\n" if divergence in {"native", "both"} else "oracle\n"
    actual_wasm = "wrong\n" if divergence in {"wasm", "both"} else "oracle\n"
    monkeypatch.setattr(parity, "require_verified_subset_target", lambda _: target)
    monkeypatch.setattr(parity, "require_wasm_toolchain", lambda: None)
    monkeypatch.setattr(
        parity,
        "development_artifact_env",
        lambda *args: {"MOLT_EXT_ROOT": str(tmp_path)},
    )
    monkeypatch.setattr(parity, "_test_env", lambda _: {})
    monkeypatch.setattr(
        parity, "_run_cpython_oracle", lambda *args: Outputs("oracle\n", "", 0)
    )

    def extension(
        root: Path,
        env: dict[str, str],
        *,
        target: str,
        target_python: TargetPythonVersion,
    ) -> None:
        extension_targets.append((target, target_python))

    def run(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        if "--output" in argv:
            assert argv[argv.index("--python-version") + 1] == short
            return subprocess.CompletedProcess(argv, 0, "build log", "compiler log")
        assert len(argv) == 1, "native comparison must execute the artifact"
        return subprocess.CompletedProcess(argv, 0, actual_native, "")

    def wasm_build(
        root: Path, program: Path, output: Path, *, extra_args: list[str]
    ) -> Path:
        assert program == parity.PROGRAM
        assert extra_args == ["--python-version", short]
        return output / "observer.wasm"

    monkeypatch.setattr(parity, "_build_extension", extension)
    monkeypatch.setattr(parity, "run_native_test_process", run)
    monkeypatch.setattr(parity, "build_wasm_linked", wasm_build)
    monkeypatch.setattr(
        parity,
        "run_wasm_linked",
        lambda *args: subprocess.CompletedProcess([], 0, actual_wasm, ""),
    )
    if divergence == "none":
        parity.test_finally_pending_observer_native_wasm_parity(tmp_path, monkeypatch)
    else:
        with pytest.raises(
            AssertionError, match=f"vs CPython {short}: stdout mismatch"
        ):
            parity.test_finally_pending_observer_native_wasm_parity(
                tmp_path, monkeypatch
            )
    assert extension_targets == [("native", target), ("wasm", target)]


def _builder() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "pending_call_probe_builder", parity.FIXTURE / "build_cpython.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_cpython_builder_projects_existing_extension_manifest(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    builder = _builder()
    with (parity.FIXTURE / "pyproject.toml").open("rb") as source:
        manifest = tomllib.load(source)
    declaration = manifest["tool"]["molt"]["extension"]
    commands: list[str] = []
    copied: list[tuple[Path, Path]] = []

    def extension(name: str, *, sources: list[str]) -> SimpleNamespace:
        assert name == declaration["module"]
        assert sources == [
            str(parity.FIXTURE / item) for item in declaration["sources"]
        ]
        return SimpleNamespace(name=name, sources=sources)

    def distribution(attrs: dict[str, object]) -> dict[str, object]:
        assert attrs["name"] == manifest["project"]["name"]
        assert attrs["version"] == manifest["project"]["version"]
        assert set(attrs) == {"name", "version", "ext_modules"}
        return attrs

    class Build:
        def __init__(self, dist: dict[str, object]) -> None:
            self.build_lib = ""
            self.build_temp = ""

        def ensure_finalized(self) -> None:
            commands.append("finalized")

        def run(self) -> None:
            assert self.build_lib == str(tmp_path / "lib")
            assert self.build_temp == str(tmp_path / "temp")
            commands.append("run")

    monkeypatch.setattr(builder, "Extension", extension)
    monkeypatch.setattr(builder, "Distribution", distribution)
    monkeypatch.setattr(builder, "build_ext", Build)
    monkeypatch.setattr(
        builder.shutil, "copyfile", lambda src, dst: copied.append((src, dst))
    )
    builder.build(parity.FIXTURE, tmp_path)
    assert commands == ["finalized", "run"]
    package = Path(*declaration["module"].split(".")[:-1])
    assert copied == [
        (
            parity.FIXTURE / package / "__init__.py",
            tmp_path / "lib" / package / "__init__.py",
        )
    ]
