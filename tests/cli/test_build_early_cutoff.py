"""`build` finishes at once when its recorded build is current (HF-250).

A fake build function stands in for the compiler: the oracle is whether it ran
and what it read, not the record's internals.
"""

from __future__ import annotations

from molt.process_environment import ProcessEnvironment

import os
import stat
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

import pytest

from molt import build_record, input_report
from molt.file_hashing import DIGEST_RACY_WINDOW_NS
from molt.cli import build_command
from molt.cli.build_request import BuildRequest


def _fake_build(
    calls: list[str],
    *,
    project_root: Path | None = None,
    entry_module: str | None = None,
):
    def build(request: BuildRequest) -> int:
        from molt.cli.build_output_layout import (
            _resolve_build_output_layout,
            _resolve_out_dir,
        )
        from molt.cli.frontend_pipeline import _output_base_for_entry
        from molt.cli.project_roots import _find_project_root
        from molt.default_paths import _default_molt_bin

        file_path = request.file_path
        assert file_path is not None
        calls.append(file_path)
        source_path = Path(file_path).resolve()
        source = source_path.read_text(encoding="utf-8")
        root = project_root or _find_project_root(source_path)
        out_dir = _resolve_out_dir(root, request.out_dir)
        layout = _resolve_build_output_layout(
            target=request.target,
            trusted=request.trusted,
            require_linked=request.require_linked,
            linked=request.linked,
            linked_output=request.linked_output,
            emit=request.emit,
            output=request.output,
            emit_ir=request.emit_ir,
            artifacts_root=root / ".test-build",
            bin_root=out_dir or _default_molt_bin(),
            output_root=out_dir or root / "dist",
            output_base=_output_base_for_entry(
                entry_module or source_path.stem, source_path
            ),
            out_dir_path=out_dir,
            project_root=root,
        )
        assert layout.output_binary is not None
        layout.output_binary.write_text(f"binary of {source}", encoding="utf-8")
        return 0

    return build


@pytest.fixture
def project(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    root = tmp_path / "project"
    root.mkdir()
    (root / "pyproject.toml").write_text("[project]\nname = 'p'\n", encoding="utf-8")
    (root / "app.py").write_text("print(1)\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", os.fspath(tmp_path / "state"))
    monkeypatch.setenv("MOLT_BIN", os.fspath(tmp_path / "bin"))
    monkeypatch.chdir(root)
    # The fixture writes its inputs just now; a recording process that started
    # 10 s later keeps them out of the racy window, as for files written before
    # the build. tests/test_build_record.py proves the racy rule itself.
    monkeypatch.setattr(
        "molt.process_birth.current_process_started_ns",
        lambda: time.time_ns() + 10 * 10**9,
    )
    return root


def _run(
    build, project: Path, output: Path | str | None, *, first: bool = True
) -> None:
    # Each call is the first build of a new process.
    build_record._BUILDS_ENTERED = 0 if first else 1
    assert (
        build(
            BuildRequest(
                file_path=os.fspath(project / "app.py"),
                output=None if output is None else os.fspath(output),
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
    build = build_command._in_build_scope(
        build_command._with_early_cutoff(_fake_build(calls))
    )
    output = tmp_path / "out" / "app"
    output.parent.mkdir()
    from molt import dx
    from molt.cli import build_output_layout

    published = []

    def collect(env, *, cwd):
        assert output.is_file()
        assert build_output_layout._BUILD_SCOPE.get() is None
        assert cwd == project
        assert env["MOLT_EXT_ROOT"]
        published.append(len(calls))

    monkeypatch.setattr(dx, "collect_checkout_state_after_result", collect)
    _run(build, project, output)
    assert len(calls) == 1
    _run(build, project, output)
    assert len(calls) == 1, "the recorded build was current"
    assert output.read_text(encoding="utf-8") == "binary of print(1)\n"

    (project / "app.py").write_text("print(2)\n", encoding="utf-8")
    _run(build, project, output)
    assert len(calls) == 2, "an edited input rebuilds"
    assert output.read_text(encoding="utf-8") == "binary of print(2)\n"
    assert published == [1, 1, 2], "the no-child cache hit also publishes maintenance"


def _judge_changes_during_the_build(monkeypatch: pytest.MonkeyPatch) -> None:
    """Start the recording process when the fixture finished writing: its
    inputs pass the racy rule, and every change the build makes is racy."""
    written = time.time_ns()
    monkeypatch.setattr(
        "molt.process_birth.current_process_started_ns",
        lambda: written + DIGEST_RACY_WINDOW_NS,
    )
    time.sleep(0.01)


@pytest.mark.parametrize("owner", ["project", "compiler"])
@pytest.mark.parametrize("explicit_source", [False, True])
def test_ir_lease_products_do_not_hide_explicit_source_inputs(
    project: Path,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    owner: str,
    explicit_source: bool,
) -> None:
    import msgspec

    from molt import dx
    from molt.cli.backend_execution import _write_backend_ir_lease
    from molt.cli.backend_ir_transport import BackendIrProgram
    from molt.cli.cache_keys import FunctionDigests
    from molt.cli.module_source import PythonSourceSnapshot
    from molt.source_root import compiler_source_root

    scratch = tmp_path / "scratch"
    scratch.mkdir()
    monkeypatch.setenv("MOLT_SCRATCH_STORAGE", f"disk:{scratch}")
    root = project if owner == "project" else compiler_source_root()
    product_root = dx.backend_ir_lease_dir(root, os.environ)
    product_root.mkdir(parents=True)
    extra = product_root / "explicit.py"
    if explicit_source:
        extra.write_text("a = 1\n", encoding="utf-8")
    output = tmp_path / "app"
    calls: list[str] = []
    emit = _fake_build(calls)
    _judge_changes_during_the_build(monkeypatch)

    def compile_program(request: BuildRequest) -> int:
        lease = _write_backend_ir_lease(
            root, BackendIrProgram({"functions": []}, FunctionDigests())
        )
        try:
            assert msgspec.msgpack.decode(lease.read_bytes()) == {
                "function_bodies": [],
                "function_digests": b"",
            }
        finally:
            lease.unlink()
        extra_text = PythonSourceSnapshot.capture(extra).text if explicit_source else ""
        result = emit(request)
        with output.open("a", encoding="utf-8") as handle:
            handle.write(extra_text)
        return result

    build = build_command._in_build_scope(
        build_command._with_early_cutoff(compile_program)
    )
    _run(build, project, output)
    _run(build, project, output)
    assert len(calls) == 1, "reading and deleting derived IR must not force a rebuild"
    assert output.read_text(encoding="utf-8") == "binary of print(1)\n" + (
        "a = 1\n" if explicit_source else ""
    )
    if explicit_source:
        extra.write_text("a = 2\n", encoding="utf-8")
        _run(build, project, output)
        assert len(calls) == 2, (
            "an explicit source remains an input inside a product root"
        )
        assert output.read_text(encoding="utf-8") == "binary of print(1)\na = 2\n"


@pytest.mark.parametrize(
    "query,reusable",
    [
        ("positive", True),
        ("negative", False),
        ("missing-parent", False),
        ("deep-missing-parent", False),
        ("file-parent", False),
        ("listing", False),
    ],
)
def test_import_fact_scope_controls_unrelated_parent_churn(
    project: Path,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    query: str,
    reusable: bool,
) -> None:
    from molt.cli.python_import_topology import PythonImportTopology

    output = tmp_path / "out" / "app"
    output.parent.mkdir()
    (tmp_path / "blocking-file").write_text("not a directory", encoding="utf-8")
    _judge_changes_during_the_build(monkeypatch)
    calls: list[str] = []
    emit = _fake_build(calls)
    boundaries: list[Path] = []
    report_directory = input_report.report_directory

    def capture_boundary(path: Path | str) -> None:
        boundaries.append(Path(path))
        report_directory(path)

    monkeypatch.setattr(input_report, "report_directory", capture_boundary)

    def compile_with_import_fact(request: BuildRequest) -> int:
        topology = PythonImportTopology()
        reports_before = len(boundaries)
        if query == "positive":
            observed = topology.probe(project)
            assert observed is not None and observed.path == project
        elif query in {
            "negative",
            "missing-parent",
            "deep-missing-parent",
            "file-parent",
        }:
            selected = {
                "negative": tmp_path / "absent-root",
                "missing-parent": tmp_path / "absent-parent" / "child",
                "deep-missing-parent": tmp_path.joinpath(
                    *(f"missing-{i}" for i in range(32)), "child"
                ),
                "file-parent": tmp_path / "blocking-file" / "child",
            }[query]
            assert topology.probe(selected) is None
            if query == "deep-missing-parent":
                assert boundaries[reports_before:] == [tmp_path], (
                    "one existing boundary, independent of missing depth"
                )
        else:
            assert "project" in topology.directory_names(tmp_path)
        # The queried root and source stay unchanged; a sibling product appears.
        (tmp_path / f"unrelated-{len(calls)}.txt").write_text(
            "product", encoding="utf-8"
        )
        topology.validate()
        return emit(request)

    build = build_command._with_early_cutoff(compile_with_import_fact)
    _run(build, project, output)
    _run(build, project, output)
    assert len(calls) == (1 if reusable else 2)
    assert output.read_text(encoding="utf-8") == "binary of print(1)\n"


@pytest.mark.parametrize("change", ["kind", "symlink-target"])
def test_a_changed_positive_import_fact_rebuilds(
    project: Path,
    tmp_path: Path,
    change: str,
) -> None:
    from molt.cli.python_import_topology import PythonImportTopology

    selected = tmp_path / "selected"
    first, second = tmp_path / "first", tmp_path / "second"
    if change == "kind":
        selected.write_text("module", encoding="utf-8")
    else:
        first.mkdir()
        second.mkdir()
        try:
            selected.symlink_to(first, target_is_directory=True)
        except OSError as exc:
            pytest.skip(f"host cannot create import alias: {exc}")
    output = tmp_path / "out" / "app"
    output.parent.mkdir()
    calls: list[str] = []
    emit = _fake_build(calls)

    def compile_with_import_fact(request: BuildRequest) -> int:
        topology = PythonImportTopology()
        observed = topology.probe(selected)
        assert observed is not None
        topology.validate()
        result = emit(request)
        output.write_text(f"{observed.path}:{observed.mode}", encoding="utf-8")
        return result

    build = build_command._with_early_cutoff(compile_with_import_fact)
    _run(build, project, output)
    original = output.read_text(encoding="utf-8")
    _run(build, project, output)
    assert len(calls) == 1

    selected.unlink()
    if change == "kind":
        selected.mkdir()
    else:
        selected.symlink_to(second, target_is_directory=True)
    _run(build, project, output)
    assert len(calls) == 2, "the consumed type or target changed"
    assert output.read_text(encoding="utf-8") == f"{selected.resolve()}:{stat.S_IFDIR}"
    assert output.read_text(encoding="utf-8") != original
    _run(build, project, output)
    assert len(calls) == 2, "the new fact is independently reusable"


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


@pytest.mark.parametrize("coordinate", ["default", "relative", "directory", "trailing"])
def test_actual_output_selection_cuts_off_ordinary_builds(
    project: Path, tmp_path: Path, coordinate: str
) -> None:
    calls: list[str] = []
    build = build_command._with_early_cutoff(_fake_build(calls))
    if coordinate == "default":
        output = None
        expected = tmp_path / "bin" / "app_molt"
    elif coordinate == "relative":
        output = "program"
        expected = project / "program"
    else:
        output = "products" + (os.sep if coordinate == "trailing" else "")
        if coordinate == "directory":
            (project / "products").mkdir()
        expected = project / "products" / "app_molt"
    _run(build, project, output)
    _run(build, project, output)
    assert calls == [str(project / "app.py")]
    assert expected.read_text(encoding="utf-8") == "binary of print(1)\n"
    (project / "app.py").write_text("print(2)\n", encoding="utf-8")
    _run(build, project, output)
    assert len(calls) == 2
    assert expected.read_text(encoding="utf-8") == "binary of print(2)\n"


def test_package_name_and_relative_out_dir_use_actual_layout(
    project: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    package = project / "package"
    package.mkdir()
    source = package / "__main__.py"
    source.write_text("print('package')\n", encoding="utf-8")
    nested = project / "nested"
    nested.mkdir()
    monkeypatch.chdir(nested)
    calls: list[str] = []
    build = build_command._with_early_cutoff(
        _fake_build(calls, project_root=project, entry_module="package.__main__")
    )
    request = BuildRequest(file_path=str(source), out_dir="products")
    for _ in range(2):
        build_record._BUILDS_ENTERED = 0
        assert build(request) == 0
    expected = project / "products" / "package_molt"
    assert calls == [str(source)]
    assert expected.read_text(encoding="utf-8") == "binary of print('package')\n"
    assert not (nested / "products").exists()


def test_a_file_output_replaced_by_a_directory_selects_a_new_binary(
    project: Path,
) -> None:
    calls: list[str] = []
    build = build_command._with_early_cutoff(_fake_build(calls))
    _run(build, project, "program")
    (project / "program").unlink()
    (project / "program").mkdir()
    _run(build, project, "program")
    assert len(calls) == 2
    assert (project / "program" / "app_molt").is_file()


@pytest.mark.parametrize("option", ["diagnostics", "cache_report"])
def test_requests_for_additional_results_still_do_not_cut_off(
    project: Path, option: str
) -> None:
    request = BuildRequest(file_path=str(project / "app.py"), **{option: True})
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


def test_input_capture_never_cuts_off_a_fresh_observation(
    project, tmp_path, monkeypatch
):
    monkeypatch.setattr(build_record, "_BUILDS_ENTERED", 0)
    calls = []
    build = build_command._with_early_cutoff(_fake_build(calls))
    request = BuildRequest(
        file_path=os.fspath(project / "app.py"),
        output=os.fspath(tmp_path / "app"),
        capture_link_inputs=True,
    )
    for _ in range(2):
        build_record._BUILDS_ENTERED = 0
        assert build(request) == 0
    assert len(calls) == 2, "every capture must produce fresh input observations"


def test_native_json_is_eligible_but_remains_a_distinct_request(project: Path) -> None:
    plain = build_command._early_cutoff_request(
        BuildRequest(file_path=str(project / "app.py"))
    )
    structured = build_command._early_cutoff_request(
        BuildRequest(file_path=str(project / "app.py"), json_output=True)
    )
    assert plain is not None and structured is not None and plain[0] != structured[0]


def test_json_without_a_recorded_result_still_runs_the_compiler(
    project: Path, tmp_path: Path
) -> None:
    calls = []
    build = build_command._in_build_scope(
        build_command._with_early_cutoff(_fake_build(calls))
    )
    request = BuildRequest(
        file_path=str(project / "app.py"),
        output=str(tmp_path / "app"),
        json_output=True,
    )
    for _ in range(2):
        build_record._BUILDS_ENTERED = 0
        assert build(request) == 0
    assert len(calls) == 2


def test_json_cutoff_emits_a_current_result_without_running_compiler(
    project: Path,
    tmp_path: Path,
    capsys,
) -> None:
    import json
    from molt.cli import build_output_layout, build_results

    out_dir = tmp_path / "products"
    artifacts = out_dir / ".molt_build" / "selected-configuration"
    artifacts.mkdir(parents=True)
    output = out_dir / "app"
    calls = []
    emit = _fake_build(calls)

    def compile_json(request):
        build_output_layout._claim_artifacts_root(artifacts)
        assert emit(request) == 0
        paths = [output]
        for name in ("archive", "stub", "runtime", "compiler"):
            path = artifacts / name
            path.write_text(f"compiled {name}", encoding="utf-8")
            paths.append(path)
        result = build_record.NativeBuildResult(
            build_record.NativeBuildFacts(
                None,
                str(project / "app.py"),
                False,
                False,
                (),
                (),
                "sha256:" + "a" * 64,
                None,
                "release",
                False,
                "generic",
                "default",
            ),
            *(build_record.RecordedArtifact.capture(path) for path in paths),
            (),
            None,
        )
        input_report.report_build_result(result.payload())
        return build_results._emit_reused_native_build_result(result)

    build = build_command._in_build_scope(
        build_command._with_early_cutoff(compile_json)
    )
    request = BuildRequest(
        file_path=str(project / "app.py"),
        output=str(output),
        out_dir=str(out_dir),
        json_output=True,
    )
    for _ in range(2):
        build_record._BUILDS_ENTERED = 0
        assert build(request) == 0
        payload = json.loads(capsys.readouterr().out)
        assert payload["status"] == "ok"
        assert payload["data"]["output"] == str(output)
    assert len(calls) == 1, "unchanged JSON must not enter the compiler"
    assert payload["data"]["cache"]["hit_tier"] == "build-record"
    (artifacts / "stub").unlink()
    build_record._BUILDS_ENTERED = 0
    assert build(request) == 0
    assert len(calls) == 2, "a missing promised stub must run the producer"


def _fake_wasm_build(calls: list[str], *, report_products: bool = True):
    """Publish the linked WASM family through the real layout and result owner."""

    def build(request: BuildRequest) -> int:
        from molt.cli import build_results
        from molt.cli.build_output_layout import (
            _resolve_build_output_layout,
            _resolve_out_dir,
        )
        from molt.cli.project_roots import _find_project_root

        file_path = request.file_path
        assert file_path is not None
        calls.append(file_path)
        source_path = Path(file_path).resolve()
        source = source_path.read_text(encoding="utf-8")
        root = _find_project_root(source_path)
        out_dir = _resolve_out_dir(root, request.out_dir)
        layout = _resolve_build_output_layout(
            target=request.target,
            trusted=request.trusted,
            require_linked=request.require_linked,
            linked=request.linked,
            linked_output=request.linked_output,
            emit=request.emit,
            output=request.output,
            emit_ir=request.emit_ir,
            artifacts_root=root / ".test-build",
            bin_root=out_dir or root / "bin",
            output_root=out_dir or root / "dist",
            output_base=source_path.stem,
            out_dir_path=out_dir,
            project_root=root,
        )
        module = layout.output_artifact
        linked = layout.linked_output_path
        assert linked is not None
        manifest = module.with_name("manifest.json")
        attestation = linked.with_name(linked.name + ".wasm-opt.json")
        module.write_text(f"module of {source}", encoding="utf-8")
        linked.write_text(f"linked {source}", encoding="utf-8")
        manifest.write_text(f"manifest of {source}", encoding="utf-8")
        attestation.write_text(f"optimizer for {source}", encoding="utf-8")
        if report_products:
            build_results._report_non_native_products(
                module,
                linked,
                {
                    "wasm": str(module),
                    "linked_wasm": str(linked),
                    "wasm_optimizer_attestation": str(attestation),
                    "manifest": str(manifest),
                },
            )
        return 0

    return build


def _wasm_request(project: Path, out_dir: Path, **options: Any) -> BuildRequest:
    from molt.compilation_target import TargetKind

    return BuildRequest(
        file_path=os.fspath(project / "app.py"),
        target=TargetKind.WASM,
        linked=True,
        require_linked=True,
        out_dir=os.fspath(out_dir),
        verbose=True,
        **options,
    )


def _run_wasm(build, request: BuildRequest) -> None:
    build_record._BUILDS_ENTERED = 0
    assert build(request) == 0


def test_an_unchanged_linked_wasm_build_reuses_its_product_family(
    project: Path, tmp_path: Path, capsys
) -> None:
    calls: list[str] = []
    build = build_command._in_build_scope(
        build_command._with_early_cutoff(_fake_wasm_build(calls))
    )
    out_dir = tmp_path / "web"
    request = _wasm_request(project, out_dir)
    _run_wasm(build, request)
    capsys.readouterr()
    _run_wasm(build, request)
    assert len(calls) == 1, "the recorded WASM family was current"
    assert (
        f"Successfully built {out_dir / 'output_linked.wasm'} (inputs unchanged)"
        in capsys.readouterr().err
    ), "a hit names the module a user runs"
    (project / "app.py").write_text("print(2)\n", encoding="utf-8")
    _run_wasm(build, request)
    assert len(calls) == 2, "an edited input rebuilds"
    assert (out_dir / "output_linked.wasm").read_text(encoding="utf-8") == (
        "linked print(2)\n"
    )


@pytest.mark.parametrize(
    "product",
    [
        "output.wasm",
        "output_linked.wasm",
        "manifest.json",
        "output_linked.wasm.wasm-opt.json",
    ],
)
@pytest.mark.parametrize("change", ["tamper", "remove"])
def test_each_published_wasm_product_must_still_hold_what_the_build_made(
    project: Path, tmp_path: Path, product: str, change: str
) -> None:
    calls: list[str] = []
    build = build_command._in_build_scope(
        build_command._with_early_cutoff(_fake_wasm_build(calls))
    )
    out_dir = tmp_path / "web"
    request = _wasm_request(project, out_dir)
    _run_wasm(build, request)
    _run_wasm(build, request)
    assert len(calls) == 1
    target = out_dir / product
    if change == "tamper":
        target.write_text("tampered", encoding="utf-8")
    else:
        target.unlink()
    _run_wasm(build, request)
    assert len(calls) == 2, f"a {change}d {product} must run the producer"
    assert target.read_text(encoding="utf-8") != "tampered"


def test_a_wasm_build_that_reports_no_products_never_records(
    project: Path, tmp_path: Path, capsys
) -> None:
    calls: list[str] = []
    build = build_command._in_build_scope(
        build_command._with_early_cutoff(_fake_wasm_build(calls, report_products=False))
    )
    request = _wasm_request(project, tmp_path / "web")
    _run_wasm(build, request)
    assert "Build record not written: WASM products were not reported" in (
        capsys.readouterr().err
    )
    _run_wasm(build, request)
    assert len(calls) == 2, "an unverified family is never reused"


@pytest.mark.parametrize(
    "options",
    [
        {"json_output": True},
        {"split_runtime": True},
        {"precompile": True},
        {"linked_output": "named_linked.wasm"},
    ],
)
def test_wasm_requests_beyond_the_verified_family_never_cut_off(
    project: Path, tmp_path: Path, options: dict[str, Any]
) -> None:
    request = _wasm_request(project, tmp_path / "web", **options)
    assert build_command._early_cutoff_request(request) is None


@pytest.mark.parametrize(
    "target", ["wasm-freestanding", "wasm32-wasip1", "wasm32-unknown-unknown"]
)
def test_only_the_default_wasm_target_is_a_verified_cutoff_cell(
    project: Path, tmp_path: Path, target: str
) -> None:
    from molt.compilation_target import TargetKind

    request = BuildRequest(
        file_path=os.fspath(project / "app.py"),
        target=TargetKind(target),
        linked=True,
        out_dir=os.fspath(tmp_path / "web"),
    )
    assert build_command._early_cutoff_request(request) is None
    default = _wasm_request(project, tmp_path / "web")
    assert build_command._early_cutoff_request(default) is not None
