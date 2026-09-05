from __future__ import annotations

import importlib.util
from pathlib import Path
import sys

import pytest


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
