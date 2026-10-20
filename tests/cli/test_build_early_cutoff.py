"""`build` finishes at once when its recorded build is current (HF-250).

A fake build function stands in for the compiler: the oracle is whether it ran
and what it read, not the record's internals.
"""

from __future__ import annotations

from molt.process_environment import ProcessEnvironment

import inspect
import os
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

import pytest

from molt import build_record
from molt.file_hashing import DIGEST_RACY_WINDOW_NS
from molt.cli import build_command


def _fake_build(calls: list[str]):
    def build(
        file_path: str | None,
        target: str = "native",
        type_hint_policy: str = "check",
        type_facts_path: str | None = None,
        pgo_profile: str | None = None,
        runtime_feedback: str | None = None,
        output: str | None = None,
        json_output: bool = False,
        verbose: bool = False,
        capabilities: Any = None,
        cache: bool = True,
        cache_dir: str | None = None,
        cache_report: bool = False,
        sysroot: str | None = None,
        emit_ir: str | None = None,
        emit: str | None = None,
        out_dir: str | None = None,
        linked: bool = False,
        linked_output: str | None = None,
        diagnostics: bool | None = None,
        diagnostics_file: str | None = None,
        portable: bool = False,
        snapshot: bool = False,
        split_runtime: bool = False,
        capability_manifest: str | None = None,
        audit_log: str | None = None,
        bolt: bool = False,
        fact_graph_request: Any = None,
        codegen_backend: str = build_command.DEFAULT_CODEGEN_BACKEND,
    ) -> int:
        assert file_path is not None and output is not None
        calls.append(file_path)
        source = Path(file_path).read_text(encoding="utf-8")
        Path(output).write_text(f"binary of {source}", encoding="utf-8")
        return 0

    return build


@pytest.fixture
def project(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    root = tmp_path / "project"
    root.mkdir()
    (root / "pyproject.toml").write_text("[project]\nname = 'p'\n", encoding="utf-8")
    (root / "app.py").write_text("print(1)\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", os.fspath(tmp_path / "state"))
    monkeypatch.chdir(root)
    # The fixture writes its inputs just now; a recording process that started
    # 10 s later keeps them out of the racy window, as for files written before
    # the build. tests/test_build_record.py proves the racy rule itself.
    monkeypatch.setattr(
        "molt.process_birth.current_process_started_ns",
        lambda: time.time_ns() + 10 * 10**9,
    )
    return root


def _run(build, project: Path, output: Path, *, first: bool = True) -> None:
    # Each call is the first build of a new process.
    build_record._BUILDS_ENTERED = 0 if first else 1
    assert (
        build(os.fspath(project / "app.py"), output=os.fspath(output), verbose=True)
        == 0
    )


def test_an_unchanged_build_reuses_its_output(
    project: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(build_record, "_BUILDS_ENTERED", 0)
    calls: list[str] = []
    build = build_command._with_early_cutoff(_fake_build(calls))
    output = tmp_path / "out" / "app"
    output.parent.mkdir()
    _run(build, project, output)
    assert len(calls) == 1
    _run(build, project, output)
    assert len(calls) == 1, "the recorded build was current"
    assert output.read_text(encoding="utf-8") == "binary of print(1)\n"

    (project / "app.py").write_text("print(2)\n", encoding="utf-8")
    _run(build, project, output)
    assert len(calls) == 2, "an edited input rebuilds"
    assert output.read_text(encoding="utf-8") == "binary of print(2)\n"


def _judge_changes_during_the_build(monkeypatch: pytest.MonkeyPatch) -> None:
    """Start the recording process when the fixture finished writing: its
    inputs pass the racy rule, and every change the build makes is racy."""
    written = time.time_ns()
    monkeypatch.setattr(
        "molt.process_birth.current_process_started_ns",
        lambda: written + DIGEST_RACY_WINDOW_NS,
    )
    time.sleep(0.01)


def test_temporary_files_leave_the_recorded_build_current(
    project: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A temporary file the build creates and removes in the system temporary
    # directory would change that input directory during the build.
    monkeypatch.setattr(build_record, "_BUILDS_ENTERED", 0)
    _judge_changes_during_the_build(monkeypatch)
    calls: list[str] = []
    fake = _fake_build(calls)

    def build_with_temporary_file(*args: Any, **kwargs: Any) -> int:
        with tempfile.NamedTemporaryFile() as scratch:
            scratch.write(b"intermediate")
        with tempfile.TemporaryDirectory():
            pass
        return fake(*args, **kwargs)

    build_with_temporary_file.__signature__ = inspect.signature(fake)  # type: ignore[attr-defined]
    build = build_command._with_early_cutoff(build_with_temporary_file)
    output = tmp_path / "app"
    _run(build, project, output)
    _run(build, project, output)
    assert len(calls) == 1, "the build's temporary files are products"


def test_a_guarded_child_leaves_the_recorded_build_current(
    project: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Cargo, the backend and the linker run under the memory guard. Each run
    # leases a scratch generation, writes custody markers and removes the
    # generation when the child ends; none of it is an input of the binary.
    from molt import process_guard

    # Admit the guard's lazy imports before observing the build. Each later
    # call still leases and retires real scratch, which is this oracle's input.
    # Windows prefixed-bytecode source admission has its own independent tests;
    # warming the shared path keeps interpreter globals and peer threads intact.
    admitted = process_guard.run_completed_command(
        [sys.executable, "-c", "pass"], env=ProcessEnvironment(os.environ)
    )
    assert admitted.returncode == 0
    monkeypatch.setattr(build_record, "_BUILDS_ENTERED", 0)
    _judge_changes_during_the_build(monkeypatch)
    calls: list[str] = []
    fake = _fake_build(calls)

    def guarded_build(*args: Any, **kwargs: Any) -> int:
        child = process_guard.run_completed_command(
            [sys.executable, "-c", "pass"], env=ProcessEnvironment(dict(os.environ))
        )
        assert child.returncode == 0
        return fake(*args, **kwargs)

    guarded_build.__signature__ = inspect.signature(fake)  # type: ignore[attr-defined]
    build = build_command._with_early_cutoff(guarded_build)
    output = tmp_path / "app"
    _run(build, project, output)
    _run(build, project, output)
    assert len(calls) == 1, "the guard's custody is not an input"


def test_a_changed_output_or_another_name_rebuilds(
    project: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(build_record, "_BUILDS_ENTERED", 0)
    calls: list[str] = []
    build = build_command._with_early_cutoff(_fake_build(calls))
    output = tmp_path / "app"
    _run(build, project, output)
    output.write_text("tampered", encoding="utf-8")
    _run(build, project, output)
    assert len(calls) == 2, "the output no longer holds what the build made"
    _run(build, project, tmp_path / "renamed")
    assert len(calls) == 3, "the linker signs the binary under its name"


def test_a_later_build_in_one_process_never_records(
    project: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(build_record, "_BUILDS_ENTERED", 0)
    calls: list[str] = []
    build = build_command._with_early_cutoff(_fake_build(calls))
    output = tmp_path / "app"
    _run(build, project, output, first=False)
    _run(build, project, output, first=False)
    assert len(calls) == 2, "process-global caches may hide a later build's reads"


def test_a_directory_output_never_cuts_off(
    project: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(build_record, "_BUILDS_ENTERED", 0)
    arguments = {
        name: parameter.default
        for name, parameter in __import__("inspect")
        .signature(_fake_build([]))
        .parameters.items()
    }
    arguments.update(file_path=os.fspath(project / "app.py"))
    for output in (os.fspath(tmp_path) + os.sep, os.fspath(tmp_path)):
        arguments["output"] = output
        assert build_command._early_cutoff_request(arguments) is None
