"""`build` finishes at once when its recorded build is current (HF-250).

A fake build function stands in for the compiler: the oracle is whether it ran
and what it read, not the record's internals.
"""

from __future__ import annotations

from molt.process_environment import ProcessEnvironment

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
from molt.cli.build_request import BuildRequest


def _fake_build(calls: list[str]):
    def build(request: BuildRequest) -> int:
        file_path, output = request.file_path, request.output
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
        build(
            BuildRequest(
                file_path=os.fspath(project / "app.py"),
                output=os.fspath(output),
                verbose=True,
            )
        )
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
    for output in (os.fspath(tmp_path) + os.sep, os.fspath(tmp_path)):
        request = BuildRequest(file_path=os.fspath(project / "app.py"), output=output)
        assert build_command._early_cutoff_request(request) is None


@pytest.mark.parametrize(
    "target,options,error",
    [
        *[
            (
                target,
                {"trusted": True},
                "Trusted mode is not supported for wasm targets",
            )
            for target in (
                "wasm",
                "wasm-freestanding",
                "wasm32-wasip1",
                "wasm32-unknown-unknown",
            )
        ],
        (
            "native",
            {"require_linked": True},
            "--require-linked is only supported for wasm targets",
        ),
        ("native", {"linked_output": "out"}, "--linked-output requires --linked"),
        (
            "native",
            {"linked": True},
            "Linked output is only supported for wasm targets",
        ),
        ("wasm", {"emit": "obj"}, "Invalid emit mode for wasm target: obj"),
        ("native", {"emit": "wasm"}, "emit=wasm requires --target wasm"),
        ("rust", {"emit": "jit"}, "emit=jit requires the native target"),
        ("luau", {"emit": "jit"}, "emit=jit requires the native target"),
        ("mlir", {"emit": "jit"}, "emit=jit requires the native target"),
        (
            "native",
            {"emit": "jit", "split_runtime": True},
            "emit=jit publishes no linked or split artifact",
        ),
    ],
)
def test_invalid_output_options_refuse_before_build_scope(
    target, options, error, tmp_path, monkeypatch, capsys
):
    import json
    from molt.compilation_target import TargetKind
    from molt.cli import build_output_layout

    def forbidden_scope():
        pytest.fail("invalid request entered artifact custody before refusal")

    monkeypatch.setattr(build_output_layout, "build_scope", forbidden_scope)
    # An absent source must not mask an option contradiction or trigger any
    # input/cache/compiler work. Invoke the actual public build boundary.
    result = build_command.build(
        BuildRequest(
            file_path=str(tmp_path / "absent.py"),
            target=TargetKind(target),
            json_output=True,
            **options,
        )
    )
    assert result == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["command"] == "build" and payload["status"] == "error"
    assert payload["errors"] == [error]
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize(
    "target,options,linked,mode",
    [
        ("native", {"trusted": True}, False, "bin"),
        ("native", {"emit": "obj"}, False, "obj"),
        ("native", {"emit": "jit"}, False, "jit"),
        ("wasm", {"linked_output": "linked.wasm"}, True, "wasm"),
        ("wasm-freestanding", {"require_linked": True}, True, "wasm"),
        ("wasm32-wasip1", {"split_runtime": True}, True, "wasm"),
        ("wasm32-unknown-unknown", {}, False, "wasm"),
        ("rust", {"linked": True}, True, "bin"),
        ("luau", {"linked": True}, True, "bin"),
        ("mlir", {}, False, "bin"),
    ],
)
def test_preflight_preserves_target_output_layout(
    target, options, linked, mode, tmp_path, monkeypatch
):
    from molt.compilation_target import TargetKind
    from molt.cli.build_output_layout import _resolve_build_output_layout

    monkeypatch.setenv("MOLT_WASM_LINKED", "0")
    arguments = dict(
        target=TargetKind(target),
        trusted=False,
        split_runtime=False,
        require_linked=False,
        linked=False,
        linked_output=None,
        emit=None,
    )
    arguments.update(options)
    layout = _resolve_build_output_layout(
        **arguments,
        output=None,
        emit_ir=None,
        artifacts_root=tmp_path / "artifacts",
        bin_root=tmp_path / "bin",
        output_root=tmp_path,
        output_base="program",
        out_dir_path=None,
        project_root=tmp_path,
    )
    assert (layout.linked, layout.emit_mode) == (linked, mode)
