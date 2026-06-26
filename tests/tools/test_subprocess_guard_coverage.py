from __future__ import annotations

import importlib.util
from pathlib import Path
import sys


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
        "def close(proc):\n"
        "    proc.terminate()\n"
        "    proc.kill()\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert [item.method for item in audit.unexpected] == [
        "process.terminate",
        "process.kill",
    ]


def test_unclassified_shell_pkill_string_fails(tmp_path: Path) -> None:
    module = _load_audit_tool()
    source = tmp_path / "bad_shell.py"
    source.write_text(
        "def script():\n    return 'pkill -f molt-backend'\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert len(audit.unexpected) == 1
    assert audit.unexpected[0].path == "bad_shell.py"
    assert audit.unexpected[0].qualname == "script"
    assert audit.unexpected[0].method == "shell.kill"


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
