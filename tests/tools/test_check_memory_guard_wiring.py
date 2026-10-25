from __future__ import annotations

import ast
import os
from pathlib import Path
import warnings

import pytest

from tools import check_memory_guard_wiring as audit_tool
from tools import check_subprocess_guard_coverage as subprocess_audit


def test_memory_guard_wiring_audit_passes_current_repo() -> None:
    audit = audit_tool.audit_repo()

    assert audit.ok is True


def test_memory_guard_wiring_audit_reports_missing_python_token(
    tmp_path: Path,
) -> None:
    path = tmp_path / "tools" / "runner.py"
    path.parent.mkdir(parents=True)
    path.write_text("print('unguarded')\n", encoding="utf-8")
    contract = audit_tool.TokenContract(
        "tools/runner.py",
        ("harness_memory_guard",),
        "test runner must import shared guard",
    )

    audit = audit_tool.audit_repo(
        tmp_path,
        python_contracts=(contract,),
        shell_contracts=(),
        required_sentinel_tokens=(),
        scanner_tokens=(),
        sentinel_tokens=(),
    )

    assert audit.ok is False
    assert audit.missing_paths == ()
    assert audit.missing_tokens == (
        audit_tool.MissingToken(
            "tools/runner.py",
            "harness_memory_guard",
            "test runner must import shared guard",
        ),
    )


def test_memory_guard_wiring_audit_reports_missing_shell_wrapper(
    tmp_path: Path,
) -> None:
    contract = audit_tool.TokenContract(
        "bench/run_all.sh",
        ("tools/guarded_exec.py",),
        "benchmark shell must enter guard",
    )

    audit = audit_tool.audit_repo(
        tmp_path,
        python_contracts=(),
        shell_contracts=(contract,),
        required_sentinel_tokens=(),
        scanner_tokens=(),
        sentinel_tokens=(),
    )

    assert audit.ok is False
    assert audit.missing_paths == (
        audit_tool.MissingPath(
            "bench/run_all.sh",
            "benchmark shell must enter guard",
        ),
    )


def test_memory_guard_wiring_audit_reports_sentinel_scanner_drift() -> None:
    audit = audit_tool.audit_repo(
        audit_tool.REPO_ROOT,
        python_contracts=(),
        shell_contracts=(),
        required_sentinel_tokens=("/bench/harness.py", "/missing.py"),
        scanner_tokens=("/bench/harness.py", "/scanner-only.py"),
        sentinel_tokens=("/bench/harness.py", "/sentinel-only.py"),
    )

    assert audit.ok is False
    assert audit.required_sentinel_missing == ("/missing.py",)
    assert audit.sentinel_drift == (
        audit_tool.SentinelTokenDrift(
            "/scanner-only.py",
            "scanner_not_in_process_sentinel",
        ),
        audit_tool.SentinelTokenDrift(
            "/sentinel-only.py",
            "process_sentinel_not_in_scanner",
        ),
    )


def _source_audit(root: Path, *, raw_audit=None):
    return audit_tool.audit_repo(
        root,
        python_contracts=(),
        shell_contracts=(),
        required_sentinel_tokens=(),
        scanner_tokens=(),
        sentinel_tokens=(),
        subprocess_guard_audit=raw_audit,
    )


@pytest.mark.parametrize(
    ("source", "lines"),
    [
        ("alias.run_guarded(cmd, env={'X': 1})\n", (1,)),
        ("alias.ｒｕｎ＿ｇｕａｒｄｅｄ(cmd, env={'X': 1})\n", (1,)),
        ("run_guarded(cmd, env={**custody, 'X': 1})\n", ()),
        ("run_guarded(cmd, env={STATE_ROOT: 'state'})\n", ()),
        ("run_guarded(cmd, env=dict(custody))\n", ()),
        ("unrelated(cmd, env={'X': 1})\n", ()),
    ],
)
def test_minimal_environment_oracle_uses_ast_names_and_preserves_safe_forms(
    source: str, lines: tuple[int, ...]
) -> None:
    observed = subprocess_audit.PythonSourceObservation(
        source.encode("utf-8"), "tests/example.py"
    )
    assert audit_tool.minimal_guard_environment_lines(observed) == lines


def test_combined_source_audit_reads_and_parses_overlap_once(tmp_path, monkeypatch):
    path = tmp_path / "tests" / "test_shared.py"
    path.parent.mkdir()
    path.write_text(
        "import subprocess\n"
        "if __name__ == '__main__':\n"
        "    subprocess.run(['echo', 'fixture'])\n"
        "    alias.run_guarded(command, env={'X': 1})\n",
        encoding="utf-8",
    )
    reads = []
    parses = []
    read_bytes = Path.read_bytes
    parse = ast.parse

    def read(candidate):
        if candidate == path:
            reads.append(candidate)
        return read_bytes(candidate)

    def parse_source(source, filename="<unknown>", *args, **kwargs):
        if filename == "tests/test_shared.py":
            parses.append(filename)
        return parse(source, filename, *args, **kwargs)

    monkeypatch.setattr(Path, "read_bytes", read)
    monkeypatch.setattr(ast, "parse", parse_source)
    audit = _source_audit(tmp_path)

    assert len(reads) == len(parses) == 1
    assert [(item.path, item.line) for item in audit.direct_test_guard_missing] == [
        ("tests/test_shared.py", 2)
    ]
    assert audit.minimal_guard_environments == (
        audit_tool.MinimalGuardEnvironment("tests/test_shared.py", 4),
    )
    assert [
        (item.path, item.line, item.method)
        for item in audit.subprocess_guard_unexpected
    ] == [("tests/test_shared.py", 3, "run")]


def test_minimal_environment_findings_drive_ok_and_both_diagnostics(tmp_path):
    # Use the real raw-call policy over an explicitly empty inventory so this
    # bounded oracle isolates the minimal-environment finding from allowlists.
    empty_raw = subprocess_audit.audit_paths(
        [], root=tmp_path, allowlist=(), text_paths=()
    )
    path = tmp_path / "tests" / "test_literal.py"
    path.parent.mkdir()
    path.write_text("alias.run_guarded(command, env={'X': 1})\n", encoding="utf-8")
    audit = _source_audit(tmp_path, raw_audit=empty_raw)
    assert not audit.ok
    assert audit_tool._audit_to_dict(audit)["minimal_guard_environments"] == [
        {"path": "tests/test_literal.py", "line": 1}
    ]
    assert "tests/test_literal.py:1" in audit_tool._format_text(audit)
    assert "guard_custody_env()" in audit_tool._format_text(audit)
    path.write_text("alias.run_guarded(command, env={**custody})\n", encoding="utf-8")
    assert _source_audit(tmp_path, raw_audit=empty_raw).ok


def test_source_audit_observes_new_members_and_same_size_same_mtime_changes(tmp_path):
    directory = tmp_path / "tests"
    directory.mkdir()
    assert _source_audit(tmp_path).minimal_guard_environments == ()
    path = directory / "test_added.py"
    bad = "alias.run_guarded(command, env={'X': 1})\n"
    good = "alias.run_guarded(command, env={**env})\n"
    width = max(len(bad), len(good))
    bad = bad.rstrip("\n").ljust(width) + "\n"
    good = good.rstrip("\n").ljust(width) + "\n"
    path.write_text(bad, encoding="utf-8")
    before = path.stat()
    assert _source_audit(tmp_path).minimal_guard_environments == (
        audit_tool.MinimalGuardEnvironment("tests/test_added.py", 1),
    )
    path.write_text(good, encoding="utf-8")
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert path.stat().st_size == before.st_size
    assert _source_audit(tmp_path).minimal_guard_environments == ()
    path.unlink()
    assert _source_audit(tmp_path).minimal_guard_environments == ()


def test_newer_program_fixture_without_launcher_is_read_but_not_parsed(
    tmp_path, monkeypatch
):
    path = tmp_path / "tests" / "differential" / "newer.py"
    path.parent.mkdir(parents=True)
    path.write_text(
        "def identity[T = int](value: T):\n    return value\n", encoding="utf-8"
    )
    parse = ast.parse
    parsed = []

    def observe(source, filename="<unknown>", *args, **kwargs):
        parsed.append(filename)
        return parse(source, filename, *args, **kwargs)

    monkeypatch.setattr(ast, "parse", observe)
    audit = _source_audit(tmp_path)
    assert "tests/differential/newer.py" not in parsed
    assert audit.minimal_guard_environments == ()
    assert audit.direct_test_guard_missing == ()
    assert audit.subprocess_guard_scanned_files == 0


def test_source_audit_keeps_ignored_files_and_direct_fixture_exclusions(tmp_path):
    directory = tmp_path / "tests" / "fixtures"
    directory.mkdir(parents=True)
    (tmp_path / ".gitignore").write_text("tests/fixtures/\n", encoding="utf-8")
    path = directory / "program.py"
    path.write_text(
        "if __name__ == '__main__':\n    alias.run_guarded(command, env={'X': 1})\n",
        encoding="utf-8",
    )
    audit = _source_audit(tmp_path)
    assert audit.direct_test_guard_missing == ()
    assert audit.minimal_guard_environments == (
        audit_tool.MinimalGuardEnvironment("tests/fixtures/program.py", 2),
    )


def test_direct_guard_token_observation_is_fresh_and_order_independent(tmp_path):
    directory = tmp_path / "tests"
    directory.mkdir()
    script = directory / "a_script.py"
    script.write_text("if __name__ == '__main__':\n    pass\n", encoding="utf-8")
    assert [
        (item.path, item.line)
        for item in _source_audit(tmp_path).direct_test_guard_missing
    ] == [("tests/a_script.py", 1)]
    guard = directory / "sitecustomize.py"
    guard.write_text(
        "from molt.pytest_memory_guard_bootstrap "
        "import ensure_python_test_memory_guard\n",
        encoding="utf-8",
    )
    assert _source_audit(tmp_path).direct_test_guard_missing == ()
    guard.unlink()
    assert [
        (item.path, item.line)
        for item in _source_audit(tmp_path).direct_test_guard_missing
    ] == [("tests/a_script.py", 1)]


@pytest.mark.parametrize("action", ["error", "default", "ignore"])
def test_direct_audit_keeps_compiler_warning_classification(tmp_path, action):
    path = tmp_path / "tests" / "test_warning.py"
    path.parent.mkdir()
    path.write_bytes(rb"value = '\q'" + b"\nif __name__ == '__main__':\n    pass\n")
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter(action, SyntaxWarning)
        audit = _source_audit(tmp_path)
    assert len(audit.direct_test_guard_missing) == 1
    finding = audit.direct_test_guard_missing[0]
    assert finding.path == "tests/test_warning.py"
    assert finding.line == (0 if action == "error" else 2)
    if action == "error":
        assert finding.reason.startswith(
            "could not parse direct-executable test candidate:"
        )
        assert "invalid escape sequence" in finding.reason
    else:
        assert finding.reason.startswith("direct-executable tests require")
    if action == "default":
        assert len(caught) == 1
        assert caught[0].category is SyntaxWarning
        assert caught[0].filename == "tests/test_warning.py"
        assert caught[0].lineno == 1
    else:
        assert caught == []
