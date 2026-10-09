from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys

import pytest

from tests.process_guard_common import run_guarded_test_process


REPO_ROOT = Path(__file__).resolve().parents[2]
AUDIT_TOOL = REPO_ROOT / "tools" / "check_subprocess_guard_coverage.py"


def _load_audit_tool():
    spec = importlib.util.spec_from_file_location(
        "molt_check_subprocess_guard_coverage",
        AUDIT_TOOL,
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_current_repo_subprocess_guard_coverage_is_clean() -> None:
    module = _load_audit_tool()

    audit = module.audit_paths()

    assert audit.ok
    assert audit.unexpected == ()
    assert audit.stale_allowlist == ()
    assert audit.expanded_allowlist == ()
    assert audit.allowlist_entries == len(module.ALLOWLIST)
    assert REPO_ROOT / "src" / "molt" / "repl.py" in module.DEFAULT_TARGETS
    assert (
        REPO_ROOT / "src" / "molt" / "toolchain_identity.py" in module.DEFAULT_TARGETS
    )
    assert REPO_ROOT / "src" / "molt_accel" in module.DEFAULT_TARGETS
    assert REPO_ROOT / "packaging" in module.DEFAULT_TARGETS


def test_copied_default_targets_still_scan_guard_text_files() -> None:
    module = _load_audit_tool()

    default_audit = module.audit_paths()
    copied_audit = module.audit_paths(tuple(module.DEFAULT_TARGETS))

    assert copied_audit.ok
    assert copied_audit.scanned_files == default_audit.scanned_files
    assert copied_audit.allowlist_entries == default_audit.allowlist_entries


@pytest.fixture
def cli_root(tmp_path: Path) -> Path:
    # The actual script keeps its real allowance policy and derives REPO_ROOT
    # from this isolated fixture. Fixture subprocess expressions are only parsed.
    tool = tmp_path / "tools" / AUDIT_TOOL.name
    tool.parent.mkdir()
    tool.write_bytes(AUDIT_TOOL.read_bytes())
    return tmp_path


def _run_audit_cli(root: Path, *args: str):
    return run_guarded_test_process(
        [sys.executable, "-I", "-B", str(root / "tools" / AUDIT_TOOL.name), *args],
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=60,
    )


@pytest.mark.parametrize("json_output", [False, True])
def test_cli_selected_file_uses_only_its_allowance(
    cli_root: Path, json_output: bool
) -> None:
    source = cli_root / "tools" / "secret_guard.py"
    source.write_text(
        "import subprocess\ndef _run(cmd):\n    return subprocess.run(cmd)\n",
        encoding="utf-8",
    )
    args = ["tools/secret_guard.py", "--json"] if json_output else [str(source)]

    result = _run_audit_cli(cli_root, *args)

    assert result.returncode == 0, result.stdout + result.stderr
    if json_output:
        report = json.loads(result.stdout)
        assert report["ok"] is True
        assert report["scanned_files"] == 1
        assert report["raw_call_count"] == 1
        assert report["allowlist_entries"] == 1
        assert report["stale_allowlist"] == []
    else:
        assert result.stdout == (
            "OK: subprocess guard coverage audit passed "
            "(scanned_files=1, raw_calls=1, allowlist_entries=1)\n"
        )


def test_cli_selected_directory_scans_python_and_text_once(cli_root: Path) -> None:
    directory = cli_root / "packaging"
    directory.mkdir()
    (directory / "bootstrap.py").write_text(
        "import subprocess\n"
        "def _prepare_environment():\n"
        "    subprocess.run([])\n"
        "    subprocess.run([])\n"
        "def main():\n"
        "    subprocess.call([])\n",
        encoding="utf-8",
    )
    (directory / "clean.sh").write_text("echo 'ready'\n", encoding="utf-8")

    result = _run_audit_cli(
        cli_root, "packaging", "packaging/bootstrap.py", "packaging/clean.sh", "--json"
    )

    assert result.returncode == 0, result.stdout + result.stderr
    report = json.loads(result.stdout)
    assert report["scanned_files"] == 2
    assert report["raw_call_count"] == 3
    assert report["allowlist_entries"] == 2


@pytest.mark.parametrize("directory_scope", [False, True])
@pytest.mark.parametrize("file_exists", [False, True])
def test_cli_selected_stale_allowances_survive_removed_calls_and_files(
    cli_root: Path, directory_scope: bool, file_exists: bool
) -> None:
    relative = "packaging/bootstrap.py" if directory_scope else "tools/secret_guard.py"
    source = cli_root / relative
    source.parent.mkdir(exist_ok=True)
    if file_exists:
        source.write_text("def clean():\n    return 1\n", encoding="utf-8")
    selection = "packaging" if directory_scope else relative

    result = _run_audit_cli(cli_root, selection, "--json")

    assert result.returncode == 1, result.stdout + result.stderr
    report = json.loads(result.stdout)
    expected_count = 2 if directory_scope else 1
    assert report["allowlist_entries"] == expected_count
    assert report["scanned_files"] == int(file_exists)
    assert len(report["stale_allowlist"]) == expected_count
    assert {entry["path"] for entry in report["stale_allowlist"]} == {relative}
    assert report["unexpected"] == []
    assert report["expanded_allowlist"] == []


@pytest.mark.parametrize("expanded", [False, True])
def test_cli_selected_file_keeps_unexpected_and_expanded_failures(
    cli_root: Path, expanded: bool
) -> None:
    extra = (
        "    subprocess.run([])\n"
        if expanded
        else "def unapproved():\n    subprocess.run([])\n"
    )
    (cli_root / "tools" / "secret_guard.py").write_text(
        "import subprocess\ndef _run(cmd):\n    subprocess.run(cmd)\n" + extra,
        encoding="utf-8",
    )

    result = _run_audit_cli(cli_root, "tools/secret_guard.py", "--json")

    assert result.returncode == 1, result.stdout + result.stderr
    report = json.loads(result.stdout)
    assert report["allowlist_entries"] == 1
    assert report["stale_allowlist"] == []
    if expanded:
        assert report["unexpected"] == []
        assert len(report["expanded_allowlist"]) == 1
        assert report["expanded_allowlist"][0]["actual_count"] == 2
    else:
        assert report["expanded_allowlist"] == []
        assert [call["qualname"] for call in report["unexpected"]] == ["unapproved"]


def test_cli_selected_text_file_uses_existing_shell_audit(cli_root: Path) -> None:
    (cli_root / "cleanup.sh").write_text("pkill -f worker\n", encoding="utf-8")

    result = _run_audit_cli(cli_root, "cleanup.sh", "--json")

    assert result.returncode == 1, result.stdout + result.stderr
    report = json.loads(result.stdout)
    assert report["scanned_files"] == 1
    assert report["allowlist_entries"] == 0
    assert [call["method"] for call in report["unexpected"]] == ["shell.kill"]


def test_cli_default_scope_retains_global_stale_checks(cli_root: Path) -> None:
    result = _run_audit_cli(cli_root, "--json")

    assert result.returncode == 1, result.stdout + result.stderr
    report = json.loads(result.stdout)
    assert report["allowlist_entries"] == len(report["stale_allowlist"])
    assert "tools/bench.py" in {entry["path"] for entry in report["stale_allowlist"]}
    assert "packaging/bootstrap.py" in {
        entry["path"] for entry in report["stale_allowlist"]
    }


def test_explicit_custom_allowlist_is_not_filtered_by_selected_files(
    tmp_path: Path,
) -> None:
    module = _load_audit_tool()
    source = tmp_path / "selected.py"
    source.write_text("pass\n", encoding="utf-8")
    entry = module.AllowedRawSubprocessUse(
        "outside.py", "missing", "run", "explicit caller-owned inventory"
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=(entry,))

    assert not audit.ok
    assert audit.stale_allowlist == (entry,)
    assert audit.allowlist_entries == 1
    assert module._audit_to_dict(audit)["allowlist_entries"] == 1


def test_explicit_custom_allowlist_formats_its_actual_denominator(
    tmp_path: Path,
) -> None:
    module = _load_audit_tool()
    source = tmp_path / "selected.py"
    source.write_text("import subprocess\nsubprocess.run([])\n", encoding="utf-8")
    entry = module.AllowedRawSubprocessUse(
        "selected.py", "<module>", "run", "custom bounded probe"
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=(entry,))

    assert audit.ok
    assert module._format_text(audit) == (
        "OK: subprocess guard coverage audit passed "
        "(scanned_files=1, raw_calls=1, allowlist_entries=1)\n"
    )


def test_unclassified_raw_subprocess_call_fails(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "bad.py"
    source.write_text(
        "import subprocess\n\n"
        "def launch():\n"
        "    return subprocess.run(['python3', '-c', 'pass'])\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert len(audit.unexpected) == 1
    assert audit.unexpected[0].path == "bad.py"
    assert audit.unexpected[0].qualname == "launch"
    assert audit.unexpected[0].method == "run"


@pytest.mark.parametrize(
    ("expression", "expected"),
    [
        ('getattr(os, "killpg", None)', "os.killpg"),
        ('getattr(os, "kill")', "os.kill"),
        ('getattr(subprocess, "run")', "run"),
        ('getattr(os, "system")', "os.system"),
        ('getattr(proc, "terminate")', "process.terminate"),
        ("subprocess.Popen", "Popen"),
        ("proc.kill", "process.kill"),
    ],
)
def test_raw_callable_aliases_remain_audited(
    tmp_path: Path, expression: str, expected: str
) -> None:
    module = _load_audit_tool()
    source = tmp_path / "aliases.py"
    source.write_text(
        "import os, subprocess\n"
        "def invoke(proc):\n"
        f"    operation = {expression}\n"
        "    alias: object = operation\n"
        "    if alias is not None:\n"
        "        alias(123, 0)\n",
        encoding="utf-8",
    )
    audit = module.audit_paths([source], root=tmp_path, allowlist=())
    # Merely retrieving a capability is not a call; invoking its alias is.
    assert [(call.qualname, call.method) for call in audit.raw_calls] == [
        ("invoke", expected),
        *([("invoke", "shell.exec")] if expected == "os.system" else []),
    ]


def test_callable_alias_scope_shadowing_and_immediate_getattr(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "shadowing.py"
    source.write_text(
        "import subprocess as sp\n"
        "launch = sp.run\n"
        "def shadow(launch):\n"
        "    launch([])\n"
        "def reassigned():\n"
        "    launch = lambda _: None\n"
        "    launch([])\n"
        "def real():\n"
        "    launch(['echo safe'], shell=True)\n"
        "    getattr(sp, 'run')(['true'])\n",
        encoding="utf-8",
    )
    audit = module.audit_paths([source], root=tmp_path, allowlist=())
    assert [(call.qualname, call.method) for call in audit.raw_calls] == [
        ("real", "run"),
        ("real", "shell.exec"),
        ("real", "run"),
    ]


def test_direct_import_alias_and_constant_capability_name(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "import_alias.py"
    source.write_text(
        "import os as host\n"
        "from os import killpg as signal_group\n"
        "alias = signal_group\n"
        "name = 'killpg'\n"
        "def invoke():\n"
        "    alias(123, 0)\n"
        "    getattr(host, name, None)(123, 0)\n",
        encoding="utf-8",
    )
    audit = module.audit_paths([source], root=tmp_path, allowlist=())
    assert [call.method for call in audit.raw_calls] == ["os.killpg", "os.killpg"]


def test_unclassified_os_kill_call_fails(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "bad_kill.py"
    source.write_text(
        "import os\n\ndef terminate(pid):\n    os.kill(pid, 9)\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert len(audit.unexpected) == 1
    assert audit.unexpected[0].path == "bad_kill.py"
    assert audit.unexpected[0].qualname == "terminate"
    assert audit.unexpected[0].method == "os.kill"


def test_unclassified_process_object_signal_fails(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "bad_process_signal.py"
    source.write_text(
        "def close(proc):\n    proc.terminate()\n    proc.kill()\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert [item.method for item in audit.unexpected] == [
        "process.terminate",
        "process.kill",
    ]


def test_non_executable_process_vocabulary_is_not_a_violation(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "documentation.py"
    source.write_text(
        '"""Never invoke taskkill or pkill -f molt-backend."""\n'
        "FORBIDDEN = ('taskkill', 'pkill -f molt-backend')\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert audit.ok
    assert audit.raw_calls == ()


def test_os_system_and_popen_execution_sinks_fail(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "bad_os_shell.py"
    source.write_text(
        "import os\n"
        "from os import popen as open_pipe\n\n"
        "def launch():\n"
        "    os.system('echo unsafe')\n"
        "    open_pipe('echo unsafe')\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert [item.method for item in audit.unexpected] == [
        "os.system",
        "shell.exec",
        "os.popen",
        "shell.exec",
    ]


def test_shell_true_constant_flow_and_kill_sink_fail(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "bad_shell.py"
    source.write_text(
        "import subprocess\n\n"
        "def launch():\n"
        "    command = 'pkill -f molt-backend'\n"
        "    use_shell = True\n"
        "    return subprocess.run(command, shell=use_shell)\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert [item.method for item in audit.unexpected] == [
        "run",
        "shell.exec",
        "shell.kill",
    ]


def test_inner_parameter_shadows_outer_shell_constant(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "shadowed_shell.py"
    source.write_text(
        "import subprocess\n\n"
        "USE_SHELL = True\n\n"
        "def launch(USE_SHELL):\n"
        "    return subprocess.run(['echo', 'safe'], shell=USE_SHELL)\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert [item.method for item in audit.unexpected] == ["run"]


def test_unclassified_makefile_pkill_fails(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "Makefile.pgo"
    source.write_text(
        'train:\n\tpkill -9 -f "molt-backend"\n',
        encoding="utf-8",
    )

    audit = module.audit_paths(
        [],
        root=tmp_path,
        allowlist=(),
        text_paths=[source],
    )

    assert not audit.ok
    assert len(audit.unexpected) == 1
    assert audit.unexpected[0].path == "Makefile.pgo"
    assert audit.unexpected[0].qualname == "<text>"
    assert audit.unexpected[0].method == "shell.kill"


def test_stale_allowlist_entry_fails(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "clean.py"
    source.write_text("def ok():\n    return 1\n", encoding="utf-8")
    allowlist = (
        module.AllowedRawSubprocessUse(
            "clean.py",
            "missing",
            "run",
            "stale entry should fail",
        ),
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=allowlist)

    assert not audit.ok
    assert audit.stale_allowlist == allowlist


def test_expanded_allowlist_entry_fails(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "expanded.py"
    source.write_text(
        "import subprocess\n\n"
        "def launch_twice():\n"
        "    subprocess.run(['true'])\n"
        "    subprocess.run(['true'])\n",
        encoding="utf-8",
    )
    allowlist = (
        module.AllowedRawSubprocessUse(
            "expanded.py",
            "launch_twice",
            "run",
            "one call is expected",
        ),
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=allowlist)

    assert not audit.ok
    assert len(audit.expanded_allowlist) == 1
    assert audit.expanded_allowlist[0].actual_count == 2
