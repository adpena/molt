from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools import command_execution


ROOT = Path(__file__).resolve().parents[2]


def test_executor_routes_only_bounded_metadata_to_direct_probe(monkeypatch) -> None:
    calls: list[dict[str, object]] = []

    def fake_run(_command: list[str], **kwargs: object):
        calls.append(kwargs)
        return subprocess.CompletedProcess([], 0, "", "")

    authority = SimpleNamespace(run_completed_command=fake_run)
    monkeypatch.setattr(
        command_execution,
        "_process_guard_authority",
        lambda _root: authority,
    )
    executor = command_execution.CommandExecutor.for_file(__file__)

    executor.run(["git", "status", "--porcelain"], capture_output=True, text=True)
    executor.run(["python", "tool.py"], capture_output=True, text=True)

    assert calls[0]["memory_guard_prefix"] is None
    assert calls[1]["memory_guard_prefix"] == executor.prefix


def test_executor_rebinds_loaded_molt_to_its_own_repo(monkeypatch) -> None:
    foreign = SimpleNamespace(__path__=["C:/foreign/molt"])
    monkeypatch.setitem(sys.modules, "molt", foreign)

    executor = command_execution.CommandExecutor.for_file(__file__)

    assert foreign.__path__ == [str(executor.repo_root / "src" / "molt")]


def test_owned_wait_escalates_only_its_exact_process() -> None:
    calls: list[tuple[str, float | None]] = []

    class Process:
        def wait(self, timeout=None):
            calls.append(("wait", timeout))
            if len([call for call in calls if call[0] == "wait"]) < 3:
                raise subprocess.TimeoutExpired(["owned"], timeout)
            return 9

        def terminate(self):
            calls.append(("terminate", None))

        def kill(self):
            calls.append(("kill", None))

    executor = command_execution.CommandExecutor.for_file(__file__)
    with pytest.raises(subprocess.TimeoutExpired):
        executor.wait_owned(Process(), timeout=1.0, terminate_timeout=0.5)  # type: ignore[arg-type]

    assert calls == [
        ("wait", 1.0),
        ("terminate", None),
        ("wait", 0.5),
        ("kill", None),
        ("wait", 0.5),
    ]


def test_executor_rejects_shell_text() -> None:
    executor = command_execution.CommandExecutor.for_file(__file__)
    with pytest.raises(TypeError, match="typed argv"):
        executor.run("git status")  # type: ignore[arg-type]


def test_executor_rejects_capture_output_with_explicit_stream() -> None:
    executor = command_execution.CommandExecutor.for_file(__file__)
    with pytest.raises(ValueError, match="capture_output cannot be combined"):
        executor.run(
            ["git", "status"],
            capture_output=True,
            stdout=subprocess.PIPE,
        )


def test_read_only_git_classifier_excludes_mutations() -> None:
    assert command_execution._is_bounded_metadata_probe(["git", "rev-parse", "HEAD"])
    assert command_execution._is_bounded_metadata_probe(
        ["git", "-C", "repo", "status", "--porcelain"]
    )
    assert not command_execution._is_bounded_metadata_probe(
        ["git", "commit", "-m", "message"]
    )


def test_owned_cargo_process_normalizes_wrapper_incremental_conflict(
    monkeypatch,
) -> None:
    captured: dict[str, object] = {}

    class FakePopen:
        def __init__(self, command: list[str], **kwargs: object) -> None:
            captured["command"] = command
            captured["kwargs"] = kwargs

    monkeypatch.setattr(command_execution.subprocess, "Popen", FakePopen)
    executor = command_execution.CommandExecutor.for_file(__file__)

    executor.start_owned(
        ["cargo", "metadata", "--no-deps"],
        env={"RUSTC_WORKSPACE_WRAPPER": "sccache", "CARGO_INCREMENTAL": "1"},
    )

    kwargs = captured["kwargs"]
    assert isinstance(kwargs, dict)
    assert kwargs["env"]["CARGO_INCREMENTAL"] == "0"


def test_executor_loads_process_guard_without_repo_package_importable(
    tmp_path: Path,
) -> None:
    script = tmp_path / "standalone_executor_probe.py"
    script.write_text(
        "import sys\n"
        f"tools = {str(ROOT / 'tools')!r}\n"
        f"repo_root = {str(ROOT)!r}\n"
        "assert repo_root not in sys.path\n"
        "try:\n"
        "    import molt\n"
        "except ModuleNotFoundError:\n"
        "    pass\n"
        "else:\n"
        "    raise AssertionError('molt unexpectedly importable')\n"
        "sys.path.insert(0, tools)\n"
        "import bootstrap_actionlint\n"
        "result = bootstrap_actionlint._COMMANDS.run([sys.executable, '--version'], "
        "capture_output=True, text=True, check=True)\n"
        "assert result.stdout.startswith('Python ')\n"
        "from command_execution import _process_guard_authority\n"
        "authority = _process_guard_authority(str(bootstrap_actionlint._COMMANDS.repo_root))\n"
        "normalized, applied = authority.cargo_subprocess_environment("
        "['cargo', '--version'], "
        "{'RUSTC_WORKSPACE_WRAPPER': 'sccache', 'CARGO_INCREMENTAL': '1'})\n"
        "assert normalized['CARGO_INCREMENTAL'] == '0'\n"
        "assert applied == ('sccache-disables-incremental',)\n"
        "assert authority.cargo_subprocess_environment.__module__.startswith("
        "authority.__package__)\n",
        encoding="utf-8",
    )
    env = {
        name: value
        for name, value in os.environ.items()
        if name not in {"PYTHONPATH", "PYTHONHOME"}
    }

    completed = subprocess.run(
        [sys.executable, "-S", str(script)],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )

    assert completed.returncode == 0, completed.stderr
    assert "ModuleNotFoundError" not in completed.stderr


def test_process_guard_direct_loader_has_one_sibling_policy_authority() -> None:
    source = (ROOT / "tools" / "command_execution.py").read_text(encoding="utf-8")
    process_guard_source = (ROOT / "src" / "molt" / "process_guard.py").read_text(
        encoding="utf-8"
    )

    assert "load_sibling_package_module_from_path" in source
    assert "spec_from_file_location" not in source
    assert "from .cargo_execution_policy import cargo_subprocess_environment" in (
        process_guard_source
    )


def test_process_guard_authority_is_isolated_per_worktree(tmp_path: Path) -> None:
    roots = [tmp_path / "one", tmp_path / "two"]
    for index, root in enumerate(roots):
        package = root / "src" / "molt"
        package.mkdir(parents=True)
        (package / "cargo_execution_policy.py").write_text(
            f"IDENTITY = {index}\n",
            encoding="utf-8",
        )
        (package / "process_guard.py").write_text(
            "from .cargo_execution_policy import IDENTITY\n",
            encoding="utf-8",
        )
    loaded_packages: list[str] = []
    command_execution._process_guard_authority.cache_clear()
    try:
        first = command_execution._process_guard_authority(str(roots[0]))
        second = command_execution._process_guard_authority(str(roots[1]))
        loaded_packages.extend((first.__package__, second.__package__))
        assert first.IDENTITY == 0
        assert second.IDENTITY == 1
        assert first.__package__ != second.__package__
    finally:
        command_execution._process_guard_authority.cache_clear()
        for package_name in loaded_packages:
            for module_name in tuple(sys.modules):
                if module_name == package_name or module_name.startswith(
                    f"{package_name}."
                ):
                    sys.modules.pop(module_name, None)
