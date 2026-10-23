from __future__ import annotations

import os
from pathlib import Path
import json
import re
import subprocess

import pytest

from molt.toolchain_identity import resolve_executable
from tests.executable_test_support import custody_spelling, write_mock_executable


@pytest.mark.parametrize("fixture", ["role", "workspace"])
def test_first_native_fixture_execution_owns_cold_budget_without_retry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fixture: str
) -> None:
    """A simulated 45-second first launch exceeds warm caps without sleeping."""
    from molt import toolchain_identity
    from tests import executable_test_support, process_guard_common
    from tests.cli import test_runtime_build_identity
    from tests import test_dx_run_context

    for name in (
        "MOLT_PROOF_COMMAND_TIMEOUT_SEC",
        "MOLT_PYTEST_BUILD_TIMEOUT_SEC",
        "MOLT_PYTEST_TIMEOUT_SEC",
    ):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("MOLT_BUILD_TIMEOUT_SEC", "90")
    monkeypatch.setattr(
        executable_test_support,
        "_BUILD_ENVIRONMENT",
        {"MOLT_BUILD_TIMEOUT_SEC": "90"},
    )
    launches = []
    compiled = {}
    binaries = {}
    executed = set()

    def complete(argv, **kwargs):
        command = [str(arg) for arg in argv]
        timeout = kwargs["timeout"]
        cwd = Path(kwargs.get("cwd") or tmp_path)
        driver = Path(command[0]).stem
        if driver == "rustc":
            image = Path(command[command.index("-o") + 1])
            write_mock_executable(image, b"MZnative-fixture")
            launches.append(("build", timeout))
            return subprocess.CompletedProcess(command, 0, "", "")
        if driver == "cargo":
            source = (cwd / "src/main.rs").read_bytes()
            fresh = compiled.get(cwd) == source
            compiled[cwd] = source
            binary = cwd / "workspace-target-oracle"
            binaries[str(binary)] = (cwd, source)
            launches.append(("build", timeout))
            return subprocess.CompletedProcess(
                command,
                0,
                json.dumps(
                    {
                        "reason": "compiler-artifact",
                        "executable": str(binary),
                        "fresh": fresh,
                    }
                )
                + "\n",
                "",
            )
        generation = binaries[command[0]]
        cold = generation not in executed
        launches.append(("cold" if cold else "warm", timeout))
        result = subprocess.CompletedProcess(command, 0, "", "")
        result.timed_out = timeout < (45 if cold else 2)
        if not result.timed_out:
            executed.add(generation)
            result.stdout = (
                re.search(rb'println!\("(\d+)"\)', generation[1])[1].decode() + "\n"
            )
        return result

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard, "guarded_completed_process", complete
    )
    if fixture == "role":
        role = tmp_path / executable_test_support.native_executable_name("wasm-ld")

        def probe(argv, **kwargs):
            assert argv == [str(role), "--version"]
            assert kwargs["env"] == {"PATH": str(tmp_path)}
            launches.append(("cold", kwargs["timeout"]))
            if kwargs["timeout"] < 45:
                raise subprocess.TimeoutExpired(argv, kwargs["timeout"])
            return subprocess.CompletedProcess(argv, 0, "LLD 22.1.8\n", "")

        process_guard_common.install_module_view(
            monkeypatch, "subprocess", subprocess, toolchain_identity, run=probe
        )
        test_runtime_build_identity.test_executable_identity_preserves_role_selecting_symlink(
            tmp_path, monkeypatch
        )
        assert launches == [("build", 90), ("cold", 90)]
        # A direct product probe still gets its original 30-second default.
        with pytest.raises(ValueError) as caught:
            toolchain_identity.probe_executable(
                role,
                version_arguments=(("--version",),),
                environment={"PATH": str(tmp_path)},
                label="unchanged product default",
            )
        assert isinstance(caught.value.__cause__, subprocess.TimeoutExpired)
        assert caught.value.__cause__.timeout == 30
        assert launches == [("build", 90), ("cold", 90), ("cold", 30)]
    else:
        which = test_dx_run_context.shutil.which
        process_guard_common.install_module_view(
            monkeypatch,
            "shutil",
            test_dx_run_context.shutil,
            test_dx_run_context,
            which=lambda name: "/fixture/cargo" if name == "cargo" else which(name),
        )
        test_dx_run_context.test_workspace_targets_execute_own_cargo_sources(tmp_path)
        assert launches == [
            ("build", 90),
            ("cold", 90),
            ("build", 90),
            ("cold", 90),
            ("build", 90),
            ("warm", 10),
            ("build", 90),
            ("warm", 10),
            ("build", 90),
            ("cold", 90),
            ("build", 90),
            ("warm", 10),
        ]


def test_mock_executable_preserves_bytes_and_admits_host_execution_permission(
    tmp_path: Path,
) -> None:
    path = tmp_path / "fake-compiler"
    path.write_bytes(b"before")
    path.chmod(0o600)
    assert write_mock_executable(path, b"mocked-native-image") == path
    assert path.read_bytes() == b"mocked-native-image"
    assert os.access(path, os.X_OK)
    assert resolve_executable(str(path), environment={}, label="test compiler") == path


def test_custody_spelling_lowercases_only_the_windows_anchor(tmp_path: Path) -> None:
    if os.name != "nt":
        entry = tmp_path / "Mixed Case" / "Tool"
        assert custody_spelling(entry) == str(entry)
        return
    assert custody_spelling(Path("D:/Work Dir/Tool.EXE")) == "d:\\Work Dir\\Tool.EXE"
    assert (
        custody_spelling(Path("//Build-Host/Share/Dir/Tool.exe"))
        == "\\\\build-host\\share\\Dir\\Tool.exe"
    )
