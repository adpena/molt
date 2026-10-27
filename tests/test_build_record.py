from __future__ import annotations

import errno
import hashlib
import importlib.util
import json
import multiprocessing
import os
import py_compile
import shutil
import stat
import sys
import tempfile
import time
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import pytest

from molt import build_record, input_report
from molt.build_output_selection import BuildOutputSelection
from molt.process_birth import current_process_started_ns

KEY = "k" * 64


def _age(path: Path, seconds: int = 3_600) -> None:
    past = time.time() - seconds
    os.utime(path, (past, past))


@pytest.fixture
def fresh_process(monkeypatch: pytest.MonkeyPatch) -> None:
    """Each test is a process's first build, with no recording active."""
    monkeypatch.setattr(build_record, "_BUILDS_ENTERED", 0)
    monkeypatch.setattr(input_report, "_SINK", None)
    monkeypatch.setattr(input_report, "_WORKER_LOG_DIR", None)


@pytest.fixture
def transient(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """The recorder's transient root, outside every directory a test lists."""
    return tmp_path_factory.mktemp("transient")


def _future_ledger() -> build_record.InputLedger:
    # Fixture files change now; a start 10 s ahead keeps them out of the racy
    # window, as if they had been written long before the build.
    return build_record.InputLedger(time.time_ns() + 10 * 10**9)


def _write(
    tmp_path: Path, ledger: build_record.InputLedger, output: Path
) -> tuple[Path, str | None]:
    record = build_record.record_path(tmp_path / "state", KEY)
    selection = BuildOutputSelection.capture(
        str(output), output, out_dir=None, project_root=tmp_path
    )
    return record, build_record.write_record(record, KEY, ledger, output=selection)


def _fixture(tmp_path: Path):
    source = tmp_path / "src" / "app.py"
    source.parent.mkdir()
    source.write_text("print('a')\n", encoding="utf-8")
    tool = tmp_path / "tool.py"
    tool.write_text("X = 1\n", encoding="utf-8")
    output = tmp_path / "out" / "app"
    output.parent.mkdir()
    output.write_bytes(b"binary")
    ledger = _future_ledger()
    ledger.file(
        source,
        input_report.stat_token(source),
        hashlib.sha256(source.read_bytes()).hexdigest(),
    )
    ledger.file(tool, input_report.stat_token(tool), None)
    ledger.directory(source.parent, input_report.stat_token(source.parent))
    ledger.probe(source.parent / "app" / "__init__.py", None)
    record, reason = _write(tmp_path, ledger, output)
    assert reason is None
    return source, tool, output, record


def _current(record: Path, output: Path, key: str = KEY) -> bool:
    result = build_record.check(record, key)
    return isinstance(result, build_record.BuildRecordHit) and result.output == output


def test_an_unchanged_build_is_current(tmp_path: Path) -> None:
    _source, _tool, output, record = _fixture(tmp_path)
    assert _current(record, output)
    assert not _current(record, output, "j" * 64), "the key binds"


def test_output_selection_survives_first_publication_and_checks_directory_kind(
    tmp_path,
):
    output = tmp_path / "program"
    selection = BuildOutputSelection.capture(
        "program", tmp_path / "default", out_dir=None, project_root=tmp_path
    )
    assert not output.exists()
    output.write_bytes(b"first published binary")
    record = build_record.record_path(tmp_path / "state", KEY)
    assert (
        build_record.write_record(record, KEY, _future_ledger(), output=selection)
        is None
    )
    result = build_record.check(record, KEY)
    assert isinstance(result, build_record.BuildRecordHit) and result.output == output
    output.unlink()
    output.mkdir()
    (output / "default").write_bytes(b"other binary")
    assert isinstance(build_record.check(record, KEY), str)


def test_output_selection_detects_redirected_directory_with_identical_output_inode(
    tmp_path,
):
    first, second = tmp_path / "first", tmp_path / "second"
    first.mkdir()
    second.mkdir()
    binary = first / "app"
    binary.write_bytes(b"shared inode does not preserve destination selection")
    try:
        os.link(binary, second / "app")
        alias = tmp_path / "alias"
        alias.symlink_to(first, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"symlinks/hardlinks unavailable: {exc}")
    selection = BuildOutputSelection.capture(
        "alias", tmp_path / "app", out_dir=None, project_root=tmp_path
    )
    record = build_record.record_path(tmp_path / "state", KEY)
    assert (
        build_record.write_record(record, KEY, _future_ledger(), output=selection)
        is None
    )
    alias.unlink()
    alias.symlink_to(second, target_is_directory=True)
    assert input_report.stat_token(alias / "app") == input_report.stat_token(binary)
    assert isinstance(build_record.check(record, KEY), str)


def test_reported_output_selection_uses_the_existing_worker_fact_protocol(
    tmp_path, monkeypatch
):
    selection = BuildOutputSelection.capture(
        None, tmp_path / "app", out_dir=None, project_root=tmp_path
    )
    ledger = _future_ledger()
    monkeypatch.setattr(input_report, "_SINK", ledger)
    input_report.report_build_output(selection.to_dict())
    assert ledger.output_selection == selection
    worker_ledger = _future_ledger()
    input_report._merge_worker_fact(
        worker_ledger, ["build_output", selection.to_dict()]
    )
    assert worker_ledger.output_selection == selection
    other = BuildOutputSelection.capture(
        None, tmp_path / "other", out_dir=None, project_root=tmp_path
    )
    worker_ledger.build_output(other.to_dict())
    assert worker_ledger.refusals == ["build selected more than one native output"]


@pytest.mark.parametrize(
    "field", ["default", "out_dir", "project_root", "path", "resolved_path"]
)
def test_record_rejects_relative_output_selection_context(tmp_path, field):
    _source, _tool, _output, record = _fixture(tmp_path)
    payload = json.loads(record.read_text(encoding="utf-8"))
    payload["output"]["selection"][field] = "relative"
    record.write_text(json.dumps(payload), encoding="utf-8")
    assert isinstance(build_record.check(record, KEY), str)


def test_previous_coordinate_record_schema_is_rejected(tmp_path):
    _source, _tool, _output, record = _fixture(tmp_path)
    payload = json.loads(record.read_text(encoding="utf-8"))
    payload["schema"] = "molt-build-record-v6"
    record.write_text(json.dumps(payload), encoding="utf-8")
    assert build_record.check(record, KEY) == "no record for this request"


def test_equal_bytes_rewritten_stay_current_only_with_a_digest(tmp_path: Path) -> None:
    source, tool, output, record = _fixture(tmp_path)
    source.write_text("print('a')\n", encoding="utf-8")
    assert _current(record, output), "a touch costs a hash"
    tool.write_text("X = 1\n", encoding="utf-8")
    assert not _current(record, output)


def test_a_content_change_is_stale(tmp_path: Path) -> None:
    source, _tool, output, record = _fixture(tmp_path)
    source.write_text("print('b')\n", encoding="utf-8")
    assert build_record.check(record, KEY) == f"file {source}"


def test_a_changed_or_missing_output_is_stale(tmp_path: Path) -> None:
    _source, _tool, output, record = _fixture(tmp_path)
    with open(output, "r+b") as handle:
        handle.write(b"B")
    assert not _current(record, output), "the binary is not what the build made"
    output.unlink()
    assert not _current(record, output)


def test_a_new_shadowing_module_is_stale(tmp_path: Path) -> None:
    source, _tool, output, record = _fixture(tmp_path)
    shadow = source.parent / "app" / "__init__.py"
    shadow.parent.mkdir()
    shadow.write_text("", encoding="utf-8")
    assert not _current(record, output), "the probed path now resolves"


@pytest.mark.parametrize("kind", ["installed-manifest", "rustup", "startup-gate"])
def test_stat_only_selection_misses_when_an_absent_input_appears(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    from molt.compiler_distribution import MANIFEST_NAME, installed_compiler
    from molt.python_external_custody import _observe_startup_gate
    from molt.rust_toolchain import rustup_proxy_selection

    root = tmp_path / "project"
    root.mkdir()
    ledger = _future_ledger()
    monkeypatch.setattr(input_report, "_SINK", ledger)
    if kind == "installed-manifest":
        subject = root / MANIFEST_NAME
        assert installed_compiler(root) is None
    elif kind == "rustup":
        tool = root / ("rustc.exe" if os.name == "nt" else "rustc")
        tool.write_bytes(b"custom compiler")
        subject = root / ("rustup.exe" if os.name == "nt" else "rustup")
        assert rustup_proxy_selection(tool) is None
    else:
        monkeypatch.chdir(root)
        monkeypatch.setenv("SETUPTOOLS_USE_DISTUTILS", "local")
        subject = root / "pybuilddir.txt"
        _observe_startup_gate("setuptools-local-distutils.v1")
    monkeypatch.setattr(input_report, "_SINK", None)
    output = tmp_path / "app"
    output.write_bytes(b"compiled output")
    record, reason = _write(tmp_path, ledger, output)
    assert reason is None and _current(record, output)
    subject.write_bytes(b"new selection")
    assert not _current(record, output), "a stat-only absence is a real dependency"


def test_captured_cargo_tool_bytes_and_resolution_are_recorded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import shutil

    from molt.cli.cargo_resource_custody import CargoExecutableCustody
    from tests.executable_test_support import native_executable_donor

    tool = tmp_path / ("tool.exe" if os.name == "nt" else "tool")
    shutil.copyfile(native_executable_donor(), tool)
    tool.chmod(tool.stat().st_mode | stat.S_IXUSR)
    expected = hashlib.sha256(tool.read_bytes()).hexdigest()
    ledger = _future_ledger()
    monkeypatch.setattr(input_report, "_SINK", ledger)
    captured = CargoExecutableCustody.capture("compiler", tool)
    monkeypatch.setattr(input_report, "_SINK", None)
    assert ledger.files[os.fspath(tool)][1] == expected
    output = tmp_path / "app"
    output.write_bytes(b"compiled output")
    record, reason = _write(tmp_path, ledger, output)
    assert reason is None and _current(record, output)
    with tool.open("ab") as stream:
        stream.write(b"changed compiler")
    assert not _current(record, output)
    with pytest.raises(ValueError, match="changed"):
        captured.verify()


@pytest.mark.parametrize("kind", ["executable", "resource"])
def test_equal_byte_cargo_tool_alias_retargeting_cannot_reuse_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    import shutil

    from molt.cli.cargo_resource_custody import (
        CargoExecutableCustody,
        CargoResourceCustody,
        CargoResourceRoot,
    )
    from tests.executable_test_support import native_executable_donor

    first, second, alias = (tmp_path / name for name in ("first", "second", "alias"))
    shutil.copyfile(native_executable_donor(), first)
    first.chmod(first.stat().st_mode | stat.S_IXUSR)
    shutil.copyfile(first, second)
    second.chmod(second.stat().st_mode | stat.S_IXUSR)
    alias.symlink_to(first)
    ledger = _future_ledger()
    monkeypatch.setattr(input_report, "_SINK", ledger)
    captured = (
        CargoExecutableCustody.capture("compiler", alias)
        if kind == "executable"
        else CargoResourceCustody.capture((CargoResourceRoot("compiler", alias),))
    )
    monkeypatch.setattr(input_report, "_SINK", None)
    output = tmp_path / "app"
    output.write_bytes(b"compiled output")
    record, reason = _write(tmp_path, ledger, output)
    assert reason is None and _current(record, output)
    alias.unlink()
    alias.symlink_to(second)
    assert build_record.check(record, KEY) == f"lookup {alias}"
    with pytest.raises(ValueError):
        captured.verify()


def test_isolated_python_probe_logs_captured_files_and_negative_selectors(
    tmp_path: Path, transient: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.python_runtime_admission import BuildPythonAdmission

    # This source belongs to the independently selected base interpreter,
    # whether its isolated loader executes source or a bytecode cache.
    source = sys.modules["pathlib"].__file__
    assert source is not None
    subject = Path(source)
    expected = hashlib.sha256(subject.read_bytes()).hexdigest()
    environment = {
        **os.environ,
        "MOLT_BUILD_PYTHON": str(getattr(sys, "_base_executable", sys.executable)),
    }
    with BuildPythonAdmission() as admission:
        unrecorded = admission.capture(environment)
    with build_record.recording(transient) as ledger:
        with BuildPythonAdmission() as admission:
            first = admission.capture(environment)
            assert admission.capture(environment) == first
    assert first == unrecorded, "recording does not change the admitted runtime"
    assert not ledger.refusals
    assert ledger.files[os.fspath(subject)][1] == expected
    assert any(outcome is None for outcome in ledger.probes.values())
    assert any(Path(name).suffix == ".zip" for name in ledger.probes)


def test_an_admission_from_before_recording_cannot_hide_its_inputs(
    transient: Path,
) -> None:
    from molt.python_runtime_admission import BuildPythonAdmission

    environment = {
        **os.environ,
        "MOLT_BUILD_PYTHON": str(getattr(sys, "_base_executable", sys.executable)),
    }
    with BuildPythonAdmission() as admission:
        identity = admission.capture(environment)
        with build_record.recording(transient) as ledger:
            assert admission.capture(environment) == identity
    assert ledger.refusals == [
        "build Python admission does not belong to this input recording"
    ]


@pytest.mark.skipif(os.name != "nt", reason="Windows observer bootstrap")
@pytest.mark.parametrize("activation", ["sink", "worker"])
def test_cold_windows_observer_can_capture_its_first_file(
    tmp_path: Path, activation: str
) -> None:
    from tests.process_guard_common import run_custody_subject_process

    root = Path(__file__).resolve().parents[1]
    # The ordinary guarded Python bootstrap queries Windows process metadata,
    # which prewarms wintypes and hides this first-observation boundary.
    program = f"""
import sys
sys.path.insert(0, {str(root / "src")!r})
import hashlib, os, time
from pathlib import Path
from molt import build_record, file_hashing, input_report
assert 'ctypes.wintypes' not in sys.modules
assert file_hashing._windows_file_api.cache_info().currsize == 0
root = Path({str(tmp_path)!r})
subject = root / 'input.bin'
before = time.time_ns()
subject.write_bytes(b'first observed input')
logs = root / 'reads'
logs.mkdir()
ledger = build_record.InputLedger(time.time_ns() + 10**10)
if {activation!r} == 'sink':
    input_report.install_sink(ledger, str(logs))
else:
    input_report.worker_initializer(str(logs))
import ctypes.wintypes
fd = file_hashing.open_stable_read_descriptor(subject)
try:
    metadata = os.fstat(fd)
    change = file_hashing.content_change_time_ns_from_fd(fd, metadata)
    data = os.read(fd, 100)
finally:
    os.close(fd)
token = input_report.stat_token(subject)
assert token is not None and before <= change <= time.time_ns(), (token, change)
assert token[5] == change
digest = hashlib.sha256(data).hexdigest()
input_report.report_captured_file(subject, token, digest)
input_report.remove_sink()
if {activation!r} == 'worker':
    input_report.merge_worker_logs(ledger, str(logs))
assert not ledger.refusals, ledger.refusals
assert ledger.files[str(subject)] == (token, digest)
assert data == b'first observed input'
"""
    completed = run_custody_subject_process(
        [sys.executable, "-I", "-S", "-B", "-c", program],
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


@pytest.mark.skipif(os.name != "nt", reason="Windows directory size providers")
@pytest.mark.parametrize("kind", ["directory", "file"])
def test_windows_tokens_distinguish_file_length_from_directory_allocation(
    tmp_path: Path, kind: str
) -> None:
    subject = tmp_path / "subject"
    if kind == "directory":
        subject.mkdir()
    else:
        subject.write_bytes(b"payload")
    metadata = subject.stat()
    fields = list(metadata)
    fields[stat.ST_SIZE] = 4096 if metadata.st_size != 4096 else 0
    alternate = os.stat_result(fields, {"st_mtime_ns": metadata.st_mtime_ns})
    first = input_report.stat_token(subject, metadata)
    second = input_report.stat_token(subject, alternate)
    assert first is not None and second is not None
    assert (first == second) is (kind == "directory")
    if kind == "directory":
        assert first[3] == 0
    else:
        assert first[3] == len(b"payload")


def test_a_rustup_candidate_becoming_the_selected_proxy_invalidates_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.rust_toolchain import rustup_proxy_selection

    tool = tmp_path / ("rustc.exe" if os.name == "nt" else "rustc")
    proxy = tmp_path / ("rustup.exe" if os.name == "nt" else "rustup")
    tool.write_bytes(b"proxy bytes")
    proxy.write_bytes(b"other bytes")
    for path in (tool, proxy):
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
    ledger = _future_ledger()
    monkeypatch.setattr(input_report, "_SINK", ledger)
    assert rustup_proxy_selection(tool) is None
    monkeypatch.setattr(input_report, "_SINK", None)
    output = tmp_path / "app"
    output.write_bytes(b"compiled output")
    record, reason = _write(tmp_path, ledger, output)
    assert reason is None and _current(record, output)
    proxy.write_bytes(tool.read_bytes())
    selection = rustup_proxy_selection(tool)
    assert selection is not None and selection.entrypoint == proxy
    assert not _current(record, output)


def test_incomplete_previous_record_schema_cannot_reuse_output(tmp_path: Path) -> None:
    _source, _tool, output, record = _fixture(tmp_path)
    payload = json.loads(record.read_text(encoding="utf-8"))
    payload["schema"] = "molt-build-record-v5"
    record.write_text(json.dumps(payload), encoding="utf-8")
    assert not _current(record, output)


def _capture_threaded_python_tree(root: Path) -> int:
    assert sys.flags.dont_write_bytecode, (
        "spawned capture must not create import caches"
    )
    from molt.python_external_custody import _observe_startup_gate
    from molt.python_file_node_custody import (
        PythonFileCaptureContext,
        _FileNodePool,
        _stable_tree_inventory,
    )

    os.chdir(root)
    os.environ["SETUPTOOLS_USE_DISTUTILS"] = "local"
    _observe_startup_gate("setuptools-local-distutils.v1")
    with PythonFileCaptureContext(hash_workers=4) as context:
        _stable_tree_inventory(
            root,
            root_id="test-runtime",
            label="test runtime",
            pool=_FileNodePool(capture_context=context),
        )
        context.verify()
        count = context.inventory_profile()["hashed_files"]
        assert isinstance(count, int)
        return count


def test_threaded_python_capture_records_every_file_and_tree_dependency(
    tmp_path: Path, transient: Path
) -> None:
    from molt.python_environment_identity import (
        python_identity_verifier_arguments,
        PythonIdentityVerifier,
    )

    # The initializer records imports before the worker unpickles the task.
    # A cold ordinary import cache is an input lookup, not an allowed output.
    # Start this fixture read-only; CPython spawn carries the explicit -B to
    # the real capture worker. Intentional cache-writer tests remain separate.
    root = Path(__file__).resolve().parents[1]
    script = f"""
import sys
sys.path.insert(0, {str(root)!r})
sys.path.insert(0, {str(root / "src")!r})
from pathlib import Path
import pytest
from tests.test_build_record import _check_threaded_python_capture_dependencies
assert sys.flags.dont_write_bytecode
with pytest.MonkeyPatch.context() as monkeypatch:
    _check_threaded_python_capture_dependencies(
        Path(sys.argv[1]), Path(sys.argv[2]), monkeypatch
    )
"""
    with PythonIdentityVerifier() as verifier:
        prefix = verifier.prefix
        completed = verifier.run(
            [
                sys.executable,
                *python_identity_verifier_arguments(prefix),
                "-c",
                script,
                str(tmp_path),
                str(transient),
            ],
            timeout=120,
            memory_guard_prefix="MOLT_PYTEST",
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
    assert completed.returncode == 0, completed.stdout + completed.stderr


def _check_threaded_python_capture_dependencies(
    tmp_path: Path, transient: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.compiler_process import worker_context

    root = tmp_path / "runtime"
    root.mkdir()
    expected = {}
    for index in range(96):
        path = root / f"module_{index:03d}.py"
        payload = f"VALUE = {index}\n".encode()
        path.write_bytes(payload)
        expected[os.fspath(path)] = hashlib.sha256(payload).hexdigest()
    monkeypatch.setattr(
        "molt.process_birth.current_process_started_ns", lambda: time.time_ns() + 10**10
    )
    with build_record.recording(transient) as ledger:
        with ProcessPoolExecutor(
            max_workers=1,
            mp_context=worker_context(),
            initializer=input_report.worker_initializer,
            initargs=(input_report.worker_log_dir(),),
        ) as pool:
            assert pool.submit(_capture_threaded_python_tree, root).result() == len(
                expected
            )
    assert not ledger.refusals
    assert {name: ledger.files[name][1] for name in expected} == expected
    assert os.fspath(root) in ledger.directories
    assert ledger.probes[os.fspath(root / "pybuilddir.txt")] is None
    output = tmp_path / "app"
    output.write_bytes(b"compiled output")
    record, reason = _write(tmp_path, ledger, output)
    assert reason is None and _current(record, output)
    subject = root / "module_031.py"
    original = subject.read_bytes()
    subject.write_bytes(b"VALUE = 99\n")
    assert not _current(record, output)
    subject.write_bytes(original)
    assert _current(record, output), "equal content after a touch stays reusable"
    (root / "new.py").write_bytes(b"new member")
    assert not _current(record, output), "the enumerated membership is an input"


def test_a_retargeted_symlink_probe_is_stale(tmp_path: Path) -> None:
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()
    link = tmp_path / "lib"
    link.symlink_to(first)
    output = tmp_path / "app"
    output.write_bytes(b"x")
    ledger = _future_ledger()
    ledger.probe(link, input_report.resolved_probe(link))
    record, reason = _write(tmp_path, ledger, output)
    assert reason is None and _current(record, output)
    link.unlink()
    link.symlink_to(second)
    assert not _current(record, output), "same file type, new target"


def test_an_unreadable_lookup_is_a_failure_not_an_absence(tmp_path: Path) -> None:
    if os.name == "nt" or os.geteuid() == 0:
        pytest.skip("POSIX permissions as an unprivileged user")
    locked = tmp_path / "locked"
    locked.mkdir()
    inside = locked / "config.toml"
    inside.write_text("", encoding="utf-8")
    locked.chmod(0)
    try:
        outcome = input_report.resolved_probe(inside)
        assert outcome is not None and outcome[0] == "", "EACCES is not ENOENT"
        assert input_report.stat_token(inside) is None, "a stat failure never raises"
    finally:
        locked.chmod(stat.S_IRWXU)


def test_inputs_changed_after_the_process_started_are_racy(tmp_path: Path) -> None:
    started = time.time_ns()
    source = tmp_path / "copied.py"
    source.write_text("x = 1\n", encoding="utf-8")
    _age(source)  # an `rsync -a` keeps an old mtime but not the change time
    output = tmp_path / "app"
    output.write_bytes(b"x")
    ledger = build_record.InputLedger(started)
    ledger.file(source, input_report.stat_token(source), None)
    assert ledger.racy_inputs() == [os.fspath(source)]
    _record, reason = _write(tmp_path, ledger, output)
    assert reason is not None and os.fspath(source) in reason


def test_the_racy_bound_is_the_process_start(transient: Path) -> None:
    # A batch server imports the compiler long before its first build: an edit
    # in between changes code the server already runs.
    before = time.time_ns()
    with build_record.recording(transient) as ledger:
        pass
    started = current_process_started_ns()
    assert started is not None and ledger.started_ns == started
    assert ledger.started_ns < before


def test_the_process_start_comes_from_the_kernel() -> None:
    # An independent clock: the parent's own time around the child's life.
    from tests.process_guard_common import run_guarded_test_process

    before = time.time_ns()
    child = run_guarded_test_process(
        [
            sys.executable,
            "-c",
            "from molt.process_birth import current_process_started_ns as s; print(s())",
        ],
        timeout=60,
    )
    after = time.time_ns()
    assert child.returncode == 0, child.stderr
    started = int(child.stdout)
    # Linux reports boot time in whole seconds, rounded down.
    assert before - 1_100_000_000 <= started <= after


def test_refusals_and_changes_after_the_read_block_the_record(tmp_path: Path) -> None:
    source = tmp_path / "a.py"
    source.write_text("x = 1\n", encoding="utf-8")
    output = tmp_path / "app"
    output.write_bytes(b"x")
    refused = _future_ledger()
    refused.refuse("an input no authority reports")
    assert _write(tmp_path, refused, output)[1] == "an input no authority reports"
    changed = _future_ledger()
    changed.file(source, input_report.stat_token(source), None)
    source.write_text("x = 22\n", encoding="utf-8")
    assert (
        _write(tmp_path, changed, output)[1]
        == f"input changed during the build: file {source}"
    )
    twice = _future_ledger()
    twice.file(source, input_report.stat_token(source), None)
    twice.file(source, (0, 0, 0, 0, 0, 0), None)
    assert twice.refusals, "two reads of different bytes"


def test_a_damaged_record_is_never_current(tmp_path: Path) -> None:
    _source, _tool, output, record = _fixture(tmp_path)
    record.write_text("{not json", encoding="utf-8")
    assert not _current(record, output)
    record.write_text(
        json.dumps({"schema": build_record._SCHEMA, "key": KEY}), encoding="utf-8"
    )
    assert not _current(record, output)


@pytest.mark.usefixtures("fresh_process")
def test_the_audit_hook_observes_reads_listings_and_programs(
    tmp_path: Path, transient: Path
) -> None:
    data = tmp_path / "data.txt"
    data.write_text("payload", encoding="utf-8")
    with build_record.recording(transient) as ledger:
        data.read_text(encoding="utf-8")
        os.listdir(tmp_path)
        (tmp_path / "written.txt").write_text("not an input", encoding="utf-8")
        # The event `subprocess.Popen` raises before it starts a program.
        program = str(Path(getattr(sys, "_base_executable", sys.executable)).resolve())
        sys.audit("subprocess.Popen", program, [program], None, None)
    assert ("file", os.fspath(data)) in ledger.observed
    assert ("directory", os.fspath(tmp_path)) in ledger.observed
    assert ("file", os.fspath(tmp_path / "written.txt")) not in ledger.observed
    assert not any(kind == "written" for kind, _name in ledger.observed)
    assert program in ledger.files, "the started process image is an input"


@pytest.mark.usefixtures("fresh_process")
def test_a_started_program_records_its_search(tmp_path: Path, transient: Path) -> None:
    # A new tool earlier on PATH, or a retargeted link, selects another program.
    early, late, missing = tmp_path / "early", tmp_path / "late", tmp_path / "none"
    early.mkdir()
    late.mkdir()
    from tests.executable_test_support import native_executable_donor

    name = "cc.exe" if os.name == "nt" else "cc"
    tool = late / name
    shutil.copyfile(native_executable_donor(), tool)
    tool.chmod(0o755)
    search = os.pathsep.join(map(os.fspath, (missing, early, late)))
    with build_record.recording(transient) as ledger:
        input_report._report_program_selection("cc", {"PATH": search}, tmp_path)
    assert ledger.probes[os.fspath(tool)] == (os.path.realpath(tool), stat.S_IFREG)
    assert os.fspath(tool) in ledger.files
    assert os.fspath(early) in ledger.directories, "a tool added here would win"
    assert ledger.probes[os.fspath(missing)] is None, "creating it is a change"
    ledger.started_ns = time.time_ns() + 10 * 10**9
    # Windows also searches cwd. Both products stay outside that input root.
    output = transient / "app"
    output.write_bytes(b"x")
    record, reason = _write(transient, ledger, output)
    assert reason is None and _current(record, output)
    shadow = early / name
    shutil.copyfile(tool, shadow)
    shadow.chmod(0o755)
    assert not _current(record, output)


def _in_fresh_interpreter(function, *args) -> None:
    with ProcessPoolExecutor(
        max_workers=1, mp_context=multiprocessing.get_context("spawn")
    ) as pool:
        pool.submit(function, *args).result()


def test_observations_skip_excluded_roots_and_keep_bytecode_and_source(
    tmp_path: Path,
) -> None:
    _in_fresh_interpreter(_check_bytecode_observations, tmp_path)


def _check_bytecode_observations(tmp_path: Path) -> None:
    kept = tmp_path / "src" / "a.py"
    cached = tmp_path / "cache" / "entry"
    bytecode = tmp_path / "src" / "__pycache__" / "a.cpython-312.pyc"
    for path in (kept, cached, bytecode):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")
    ledger = _future_ledger()
    sys.pycache_prefix = None
    input_report._SINK = ledger
    try:
        for path in (cached, bytecode):
            input_report._audit_hook("open", (os.fspath(path), "r", 0))
    finally:
        input_report._SINK = None
    build_record.resolve_observed(ledger, (tmp_path / "cache",))
    assert set(ledger.files) == {os.fspath(kept), os.fspath(bytecode)}


@pytest.mark.parametrize(
    "start_method",
    sorted({"spawn", *(["fork"] if sys.platform != "win32" else [])}),
)
@pytest.mark.usefixtures("fresh_process")
def test_worker_reads_and_reports_merge_into_the_parent_ledger(
    tmp_path: Path, start_method: str, transient: Path
) -> None:
    data = tmp_path / "worker-input.txt"
    data.write_text("payload", encoding="utf-8")
    reported = tmp_path / "worker-reported.toml"
    reported.write_text("x = 1\n", encoding="utf-8")
    with build_record.recording(transient) as ledger:
        with ProcessPoolExecutor(
            max_workers=1,
            mp_context=multiprocessing.get_context(start_method),
            initializer=input_report.worker_initializer,
            initargs=(input_report.worker_log_dir(),),
        ) as pool:
            assert pool.submit(Path.read_text, data).result() == "payload"
            pool.submit(input_report.report_data_file, reported).result()
    assert ("file", os.fspath(data)) in ledger.observed
    assert os.fspath(reported) in ledger.files, "a worker's explicit report"
    assert ledger.probes[os.fspath(reported)] is not None
    assert not ledger.refusals


@pytest.mark.usefixtures("fresh_process")
def test_a_lost_worker_fact_refuses_the_record(tmp_path: Path, transient: Path) -> None:
    with build_record.recording(transient) as ledger:
        log_dir = input_report.worker_log_dir()
        assert log_dir is not None
        (Path(log_dir) / "4242.jsonl.failed").write_text("", encoding="utf-8")
        (Path(log_dir) / "4243.jsonl").write_text('["file"]\n', encoding="utf-8")
    assert len(ledger.refusals) == 2


@pytest.mark.usefixtures("fresh_process")
def test_only_a_process_first_build_may_record() -> None:
    # An earlier build warms process-global caches whether or not it was
    # eligible for a record.
    assert build_record.enter_build()
    assert not build_record.enter_build()


def test_the_request_key_binds_arguments_environment_cwd_and_host() -> None:
    base = {
        "arguments": {"file_path": "/a.py", "profile": "dev", "output": "/o/app"},
        "environment": {"MOLT_X": "1", "TERM_SESSION_ID": "s1"},
        "cwd": "/w",
        "interpreter": "/py",
        "interpreter_version": "3.12",
        "host": {"machine": "arm64", "cpu": "M3"},
    }

    def key(**changes):
        material = {**base, **changes}
        return build_record.request_key(
            material["arguments"],
            environment=material["environment"],
            cwd=material["cwd"],
            interpreter=material["interpreter"],
            interpreter_version=material["interpreter_version"],
            host=material["host"],
        )

    first = key()
    assert first == key(environment={"MOLT_X": "1", "TERM_SESSION_ID": "s2"})
    assert first != key(environment={"MOLT_X": "2", "TERM_SESSION_ID": "s1"})
    assert first != key(
        arguments={"file_path": "/a.py", "profile": "release", "output": "/o/app"}
    )
    assert first != key(
        arguments={"file_path": "/a.py", "profile": "dev", "output": "/o/app2"}
    ), "the linker signs the binary under its file name"
    assert first != key(cwd="/v")
    assert first != key(interpreter_version="3.13")
    assert first != key(host={"machine": "arm64", "cpu": "M4"})
    assert key(arguments={"request": object()}) is None, "not plain data"

    # Each guarded build carries a fresh guard token and scratch lease and a
    # memory budget measured at launch; none of them changes the binary.
    guarded = {
        "MOLT_X": "1",
        "MOLT_MEMORY_GUARD_TOKEN": "a" * 32,
        "MOLT_GUARD_SCRATCH_ROOT": "/s/pt-aaaaaaaa",
        "MOLT_BACKEND_MEMORY_AVAILABLE_GB": "32.35",
        "MOLT_BUILD_TIMEOUT_SEC": "600",
    }
    assert first == key(environment=guarded)
    assert first == key(
        environment={
            **guarded,
            "MOLT_MEMORY_GUARD_TOKEN": "b" * 32,
            "MOLT_GUARD_SCRATCH_ROOT": "/s/pt-bbbbbbbb",
            "MOLT_BACKEND_MEMORY_AVAILABLE_GB": "31.07",
        }
    )
    # A name the registry does not mark output-neutral stays keyed.
    assert first != key(environment={**guarded, "MOLT_BACKEND": "llvm"})


@pytest.mark.parametrize("extended_side", ["neither", "observed", "products"])
def test_only_named_products_are_excluded_and_explicit_inputs_win(
    tmp_path: Path, extended_side: str
) -> None:
    if extended_side != "neither" and os.name != "nt":
        pytest.skip("Windows verbatim path spelling")

    def observed(path: Path) -> str:
        return (
            "\\\\?\\" + str(path).upper() if extended_side == "observed" else str(path)
        )

    def selected(path: Path) -> Path:
        return (
            Path("\\\\?\\" + str(path).upper()) if extended_side == "products" else path
        )

    # Existing product custody owns the scratch root and exact published file;
    # its parent and explicitly reported inputs never become implicit products.
    source = tmp_path / "project" / "config.toml"
    scratch = tmp_path / "scratch"
    product = scratch / "object.o"
    sibling = tmp_path / "scratch-sibling" / "config.toml"
    output = tmp_path / "out" / "app"
    for path in (source, product, sibling, output):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")
    ledger = _future_ledger()
    ledger.observed |= {
        ("file", observed(source)),
        ("file", observed(product)),
        ("file", observed(sibling)),
        ("file", observed(output)),
        ("directory", observed(source.parent)),
        ("directory", observed(scratch)),
    }
    build_record.resolve_observed(
        ledger, (selected(scratch),), published_output=selected(output)
    )
    assert set(ledger.files) == {observed(source), observed(sibling)}
    assert set(ledger.directories) == {observed(source.parent)}

    explicit = _future_ledger()
    explicit.file(output, input_report.stat_token(output), None)
    explicit.file(product, input_report.stat_token(product), None)
    explicit.observed = ledger.observed.copy()
    build_record.resolve_observed(
        explicit, (selected(scratch),), published_output=selected(output)
    )
    assert {os.fspath(output), os.fspath(product)} <= explicit.files.keys()


@pytest.mark.parametrize("extended_side", ["neither", "observed", "products"])
@pytest.mark.parametrize("output_name", ["app", "MixedCase.exe"])
def test_the_outputs_publication_custody_is_a_product(
    tmp_path: Path, extended_side: str, output_name: str
) -> None:
    if extended_side != "neither" and os.name != "nt":
        pytest.skip("Windows verbatim path spelling")
    from molt import artifact_publication, file_publication

    output = tmp_path / "out" / output_name
    output.parent.mkdir()
    output.write_bytes(b"binary")
    receipt = artifact_publication.publication_receipt_path(output)
    receipt.write_text("{}", encoding="utf-8")
    other_receipt = artifact_publication.publication_receipt_path(
        output.parent / "other"
    )
    other_receipt.write_text("{}", encoding="utf-8")
    # A stage and a journal the publication created and removed again.
    stage = file_publication.staged_file_path(output, purpose="stage")
    receipt_stage = file_publication.staged_file_path(receipt)
    journal = output.parent / f".molt-artifact-publication-{'0' * 32}.json"
    ledger = _future_ledger()
    ledger.observed |= {
        ("file", os.fspath(receipt)),
        ("file", os.fspath(stage)),
        ("file", os.fspath(receipt_stage)),
        ("file", os.fspath(journal)),
        ("file", os.fspath(output.parent / ".molt-artifact-publication.lock")),
        ("file", os.fspath(other_receipt)),
        # The publication's own scan for journals.
        ("directory", os.fspath(output.parent)),
    }
    if extended_side == "observed":
        ledger.observed = {
            (kind, "\\\\?\\" + name.upper()) for kind, name in ledger.observed
        }
        other_receipt = Path("\\\\?\\" + str(other_receipt).upper())
    selected_output = (
        Path("\\\\?\\" + str(output).upper()) if extended_side == "products" else output
    )
    build_record.resolve_observed(ledger, (), published_output=selected_output)
    # Another destination's receipt stays an input; nothing records the
    # directory the publication changed.
    assert set(ledger.files) == {os.fspath(other_receipt)}
    assert ledger.directories == {} and ledger.probes == {}

    # A reader that consumes the directory reports it, and that report stays.
    explicit = _future_ledger()
    explicit.directory(output.parent, input_report.stat_token(output.parent))
    explicit.observed = {("directory", os.fspath(output.parent))}
    build_record.resolve_observed(explicit, (), published_output=selected_output)
    assert set(explicit.directories) == {os.fspath(output.parent)}


@pytest.mark.usefixtures("fresh_process")
def test_mkdir_and_failed_write_attempts_never_own_existing_inputs(
    tmp_path: Path, transient: Path
) -> None:
    source = tmp_path / "project" / "config.toml"
    source.parent.mkdir()
    source.write_text("version = 1\n", encoding="utf-8")
    with build_record.recording(transient) as ledger:
        assert source.read_text(encoding="utf-8") == "version = 1\n"
        os.listdir(source.parent)
        source.parent.mkdir(parents=True, exist_ok=True)
        with pytest.raises(FileExistsError):
            source.parent.mkdir()
        with pytest.raises((IsADirectoryError, PermissionError)):
            source.parent.open("w").close()
    build_record.resolve_observed(ledger, ())
    assert os.fspath(source) in ledger.files
    assert os.fspath(source.parent) in ledger.directories


@pytest.mark.skipif(
    os.mkdir not in os.supports_dir_fd, reason="mkdir dir_fd unavailable"
)
@pytest.mark.usefixtures("fresh_process")
def test_mkdir_relative_to_descriptor_does_not_exclude_a_cwd_decoy(
    tmp_path, monkeypatch, transient
):
    actual = tmp_path / "actual"
    decoy = tmp_path / "decoy"
    for root in (actual, decoy):
        (root / "existing").mkdir(parents=True)
    source = decoy / "existing" / "config"
    source.write_text("input", encoding="utf-8")
    monkeypatch.chdir(decoy)
    descriptor = os.open(actual, os.O_RDONLY)
    try:
        with build_record.recording(transient) as ledger:
            source.read_text(encoding="utf-8")
            with pytest.raises(FileExistsError):
                os.mkdir("existing", dir_fd=descriptor)
        build_record.resolve_observed(ledger, ())
        assert os.fspath(source) in ledger.files
    finally:
        os.close(descriptor)


@pytest.mark.parametrize(
    "mode", ["r+", "a+", "a", "read-then-write", "os-rdwr", "os-append"]
)
@pytest.mark.usefixtures("fresh_process")
def test_updates_preserve_input_reads_and_refuse_racy_records(
    tmp_path, mode, transient
):
    source = tmp_path / "input"
    source.write_text("before", encoding="utf-8")
    with build_record.recording(transient) as ledger:
        if mode.startswith("os-"):
            flags = os.O_RDWR if mode == "os-rdwr" else os.O_WRONLY | os.O_APPEND
            descriptor = os.open(source, flags)
            try:
                os.write(descriptor, b"after")
            finally:
                os.close(descriptor)
        else:
            if mode == "read-then-write":
                source.read_text(encoding="utf-8")
                mode = "w"
            with source.open(mode, encoding="utf-8") as handle:
                handle.write("after")
    build_record.resolve_observed(ledger, ())
    assert os.fspath(source) in ledger.files
    output = tmp_path / "app"
    output.write_bytes(b"binary")
    assert _write(tmp_path, ledger, output)[1] is not None


_EXISTING = b"existing bytes"
_ACCESS = {"rdonly": os.O_RDONLY, "wronly": os.O_WRONLY, "rdwr": os.O_RDWR}
_EXTRA = {
    "plain": 0,
    "append": os.O_APPEND,
    "trunc": os.O_TRUNC,
    "creat": os.O_CREAT,
    # O_EXCL without O_CREAT creates nothing, so the open still reads the old
    # bytes: Linux and macOS ignore it on a regular file, and the Windows CRT
    # applies _O_EXCL only with _O_CREAT.
    "excl": os.O_EXCL,
    "creat-trunc": os.O_CREAT | os.O_TRUNC,
    "creat-excl": os.O_CREAT | os.O_EXCL,
    "append-creat-trunc": os.O_APPEND | os.O_CREAT | os.O_TRUNC,
}


def _reads_or_appends_to_old_bytes(path: Path, flags: int) -> bool:
    """Ask the OS about one open of a file that holds bytes: does the
    descriptor read those bytes, or does its append keep them in the file?

    Each target specifies every outcome asked here. Only an exclusive create
    refuses the open (EEXIST), and only a write-only descriptor refuses the
    read (EBADF, which the Windows CRT also documents for a descriptor that
    is not open for reading).
    """
    path.write_bytes(_EXISTING)
    try:
        descriptor = os.open(path, flags | getattr(os, "O_BINARY", 0), 0o600)
    except FileExistsError:
        assert flags & (os.O_CREAT | os.O_EXCL) == os.O_CREAT | os.O_EXCL
        return False
    try:
        try:
            reads = os.read(descriptor, len(_EXISTING)) == _EXISTING
        except OSError as error:
            assert error.errno == errno.EBADF
            assert flags & (os.O_WRONLY | os.O_RDWR) == os.O_WRONLY
            reads = False
        appends = bool(flags & os.O_APPEND) and path.read_bytes() == _EXISTING
        return reads or appends
    finally:
        os.close(descriptor)


@pytest.mark.parametrize("extra", sorted(_EXTRA))
@pytest.mark.parametrize("access", sorted(_ACCESS))
def test_an_open_is_a_read_exactly_when_it_can_read_or_append_to_old_bytes(
    tmp_path: Path, access: str, extra: str
) -> None:
    flags = _ACCESS[access] | _EXTRA[extra]
    classified = input_report._reads_existing_content(flags)
    if access == "rdonly" and flags & os.O_TRUNC:
        # No target specifies this open, so the OS is no oracle: POSIX leaves
        # O_RDONLY|O_TRUNC undefined (macOS and Linux truncate), and Windows
        # maps it to TRUNCATE_EXISTING, which CreateFile documents only with
        # write access. A read stays the safe side.
        assert classified
    else:
        assert classified == _reads_or_appends_to_old_bytes(tmp_path / "file", flags)


@pytest.mark.usefixtures("fresh_process")
def test_an_open_event_without_int_flags_refuses_the_record(
    tmp_path: Path, transient: Path
) -> None:
    # Every CPython "open" event carries int flags, so no CPython producer
    # reaches this. An unknown producer's event refuses instead of dropping
    # a read the classifier cannot judge.
    with build_record.recording(transient) as ledger:
        sys.audit("open", os.fspath(tmp_path / "input"), "r", None)
    assert ledger.refusals == ["unobservable open event"]


def test_an_unnamed_temporary_file_is_no_read(tmp_path: Path) -> None:
    if sys.platform != "linux":
        pytest.skip("O_TMPFILE is Linux-only")
    flags = os.O_RDWR | os.O_TMPFILE
    descriptor = os.open(tmp_path, flags, 0o600)
    try:
        assert os.read(descriptor, 1) == b""
    finally:
        os.close(descriptor)
    assert not input_report._reads_existing_content(flags)


@pytest.mark.usefixtures("fresh_process")
def test_a_removed_temporary_file_adds_no_input(
    tmp_path: Path, transient: Path
) -> None:
    # HF-250: a guard captures its child's output in `tempfile.TemporaryFile`
    # inside its scratch lease and removes the lease before the build records.
    # Each removed capture left the lease's parent as a changing directory
    # input, so no unchanged rebuild was ever recorded.
    lease = tmp_path / "scratch" / "lease"
    lease.mkdir(parents=True)
    source = tmp_path / "project" / "removed" / "config"
    source.parent.mkdir(parents=True)
    source.write_text("input", encoding="utf-8")
    with build_record.recording(transient) as ledger:
        with tempfile.TemporaryFile(dir=lease) as capture:
            capture.write(b"child output")
            capture.seek(0)
            assert capture.read() == b"child output"
        assert source.read_text(encoding="utf-8") == "input"
    shutil.rmtree(lease)
    shutil.rmtree(source.parent)
    build_record.resolve_observed(ledger, ())
    # A removed real read still makes its surviving parent an input.
    assert os.fspath(source.parent.parent) in ledger.directories
    assert os.fspath(lease.parent) not in ledger.directories
    recorded = (*ledger.files, *ledger.directories, *ledger.probes)
    assert not [name for name in recorded if name.startswith(os.fspath(lease))]


@pytest.mark.parametrize("payload", [None, False, True, 0, 1.5, "text", [], [1, {}]])
def test_non_object_json_records_miss_without_failing_builds(tmp_path, payload):
    _source, _tool, output, record = _fixture(tmp_path)
    record.write_text(json.dumps(payload), encoding="utf-8")
    assert isinstance(build_record.check(record, KEY), str)


@pytest.mark.parametrize("field", ["files", "directories", "probes"])
@pytest.mark.parametrize(
    "value", [None, False, 42, "text", [None], [[10**100, [], None]]]
)
def test_malformed_record_collections_miss_without_raising(tmp_path, field, value):
    _source, _tool, output, record = _fixture(tmp_path)
    payload = json.loads(record.read_text(encoding="utf-8"))
    payload[field] = value
    record.write_text(json.dumps(payload), encoding="utf-8")
    assert isinstance(build_record.check(record, KEY), str)


def test_excessively_nested_json_record_misses(tmp_path):
    _source, _tool, output, record = _fixture(tmp_path)
    record.write_text("[" * 2_000 + "0" + "]" * 2_000, encoding="utf-8")
    assert isinstance(build_record.check(record, KEY), str)


@pytest.mark.skipif(not hasattr(os, "fork"), reason="fork unavailable")
def test_forked_reporter_does_not_inherit_another_threads_locked_mutex(tmp_path):
    from tests.process_guard_common import run_guarded_test_process

    script = """
import os, signal, sys, threading, time
from molt import input_report
input_report._WORKER_LOG_DIR = sys.argv[1]
held, release = threading.Event(), threading.Event()
def hold():
    with input_report._REPORTED_LOCK:
        held.set()
        release.wait()
thread = threading.Thread(target=hold)
thread.start()
assert held.wait(2)
child = os.fork()
if child == 0:
    input_report.report_data_file(sys.argv[2])
    os._exit(0)
try:
    deadline = time.monotonic() + 2
    while True:
        finished, status = os.waitpid(child, os.WNOHANG)
        if finished:
            assert os.waitstatus_to_exitcode(status) == 0
            break
        if time.monotonic() >= deadline:
            os.kill(child, signal.SIGKILL)
            os.waitpid(child, 0)
            raise AssertionError('forked reporter retained a locked parent mutex')
        time.sleep(0.01)
finally:
    release.set()
    thread.join(2)
"""
    log_root = tmp_path / "worker-logs"
    log_root.mkdir()
    data = tmp_path / "input"
    data.write_bytes(b"input")
    child = run_guarded_test_process(
        [sys.executable, "-c", script, str(log_root), str(data)], timeout=15
    )
    assert child.returncode == 0, child.stderr
    reports = list(log_root.glob("*.jsonl"))
    assert len(reports) == 1
    entries = [
        json.loads(line) for line in reports[0].read_text(encoding="utf-8").splitlines()
    ]
    assert any(entry[:2] == ["file", str(data)] for entry in entries)


def test_vanished_observations_become_probes_with_their_parent(tmp_path: Path) -> None:
    failed = tmp_path / "src" / "never_existed.cfg"
    failed.parent.mkdir()
    ledger = _future_ledger()
    ledger.observed.add(("file", os.fspath(failed)))
    build_record.resolve_observed(ledger, ())
    assert ledger.probes[os.fspath(failed)] is None, "a failed lookup is an absence"
    assert os.fspath(failed.parent) in ledger.directories, (
        "a removal during the build changes the parent, which the racy rule sees"
    )
    assert not ledger.refusals


@pytest.mark.parametrize("kind", ["file", "directory"])
def test_nested_failed_observations_keep_each_probe_and_the_existing_boundary(
    tmp_path: Path, kind: str
) -> None:
    output = tmp_path / "app"
    output.write_bytes(b"binary")
    failed = tmp_path / "missing" / "nested" / "input"
    # Publication changes its own product directory, not this input boundary.
    build_record.record_path(tmp_path / "state", KEY).parent.mkdir(parents=True)
    ledger = _future_ledger()
    ledger.observed.add((kind, os.fspath(failed)))
    build_record.resolve_observed(ledger, ())
    assert set(ledger.probes) == {
        os.fspath(failed),
        os.fspath(failed.parent),
        os.fspath(failed.parent.parent),
    }
    assert all(value is None for value in ledger.probes.values())
    assert set(ledger.directories) == {os.fspath(tmp_path)}
    record, reason = _write(tmp_path, ledger, output)
    assert reason is None and _current(record, output)
    failed.parent.mkdir(parents=True)
    if kind == "file":
        failed.write_bytes(b"new input")
    else:
        failed.mkdir()
    assert not _current(record, output), "a previously absent lookup now resolves"


def test_removed_nested_input_keeps_a_racy_boundary_even_with_explicit_probes(
    tmp_path: Path,
) -> None:
    import shutil

    output = tmp_path / "app"
    output.write_bytes(b"binary")
    failed = tmp_path / "existing" / "nested" / "input"
    failed.parent.mkdir(parents=True)
    failed.write_bytes(b"input that was read then removed")
    before = input_report.stat_token(tmp_path)
    assert before is not None
    # The start/racy threshold admits the ancestor generation before removal,
    # and must reject the actual generation produced by the subtree removal.
    ledger = build_record.InputLedger(
        before[5] + build_record.DIGEST_RACY_WINDOW_NS + 1
    )
    shutil.rmtree(tmp_path / "existing")
    after = input_report.stat_token(tmp_path)
    assert after is not None and after[5] > before[5]
    ledger.probe(failed, None)
    ledger.probe(failed.parent, None)
    ledger.observed.add(("file", os.fspath(failed)))
    build_record.resolve_observed(ledger, ())
    assert not ledger.refusals, "the lookup itself is fully reportable"
    assert os.fspath(tmp_path) in ledger.directories
    assert _write(tmp_path, ledger, output)[1] == (
        f"input changed after the recording process started: {tmp_path}"
    )


def test_failed_traversal_records_a_regular_file_boundary_as_a_file(
    tmp_path: Path,
) -> None:
    output = tmp_path / "app"
    output.write_bytes(b"binary")
    boundary = tmp_path / "not-a-directory"
    boundary.write_bytes(b"blocking file")
    failed = boundary / "nested" / "input"
    ledger = _future_ledger()
    ledger.observed.add(("file", os.fspath(failed)))
    build_record.resolve_observed(ledger, ())
    assert set(ledger.files) == {os.fspath(boundary)}
    assert ledger.directories == {}
    record, reason = _write(tmp_path, ledger, output)
    assert reason is None and _current(record, output)
    boundary.unlink()
    failed.parent.mkdir(parents=True)
    failed.write_bytes(b"now reachable")
    assert not _current(record, output)


def test_failed_lookup_refuses_an_existing_boundary_without_a_change_token(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output = tmp_path / "app"
    output.write_bytes(b"binary")
    failed = tmp_path / "missing" / "nested" / "input"
    ledger = _future_ledger()
    ledger.observed.add(("file", os.fspath(failed)))
    real_stat = build_record.stat_token

    def without_boundary_token(path, metadata=None):
        if os.fspath(path) == os.fspath(tmp_path):
            return None
        return real_stat(path, metadata)

    monkeypatch.setattr(build_record, "stat_token", without_boundary_token)
    build_record.resolve_observed(ledger, ())
    assert ledger.refusals == [f"unreportable lookup generation {tmp_path}"]
    assert _write(tmp_path, ledger, output)[1] == ledger.refusals[0]


@pytest.mark.skipif(
    os.name == "nt" or (hasattr(os, "geteuid") and os.geteuid() == 0),
    reason="requires POSIX search permissions for an unprivileged process",
)
def test_permission_failure_keeps_its_exact_probe_and_directory_boundary(
    tmp_path: Path,
) -> None:
    import errno

    output = tmp_path / "app"
    output.write_bytes(b"binary")
    boundary = tmp_path / "denied"
    boundary.mkdir()
    failed = boundary / "input"
    boundary.chmod(0)
    try:
        ledger = _future_ledger()
        ledger.observed.add(("file", os.fspath(failed)))
        build_record.resolve_observed(ledger, ())
        assert ledger.probes[os.fspath(failed)] == ("", errno.EACCES)
        assert set(ledger.directories) == {os.fspath(boundary)}
        record, reason = _write(tmp_path, ledger, output)
        assert reason is None and _current(record, output)
        boundary.chmod(0o700)
        assert not _current(record, output), "permission restoration changes the lookup"
    finally:
        boundary.chmod(0o700)


def test_sibling_failed_lookups_reuse_only_completed_ancestor_chains(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    parent = tmp_path.joinpath(*(f"missing-{i}" for i in range(24)))
    siblings = [parent / f"input-{i}" for i in range(32)]
    ledger = _future_ledger()
    for name in siblings:
        ledger.probe(name, None)
        ledger.observed.add(("file", os.fspath(name)))
    real_stat = build_record.stat_token
    calls: dict[str, int] = {}

    def count_subject_stat(path, metadata=None):
        name = os.fspath(path)
        if name == os.fspath(tmp_path) or name.startswith(os.fspath(tmp_path) + os.sep):
            calls[name] = calls.get(name, 0) + 1
        return real_stat(path, metadata)

    monkeypatch.setattr(build_record, "stat_token", count_subject_stat)
    build_record.resolve_observed(ledger, ())
    assert not ledger.refusals
    assert set(ledger.directories) == {os.fspath(tmp_path)}
    assert len(calls) == 32 + 24 + 1
    assert max(calls.values()) == 1, "shared missing ancestors are traversed once"
    assert len(ledger.probes) == 32 + 24


@pytest.mark.parametrize("transition", ["directory", "lookup"])
def test_repeated_equal_facts_are_accepted_but_changed_facts_refuse(
    tmp_path: Path, transition: str
) -> None:
    subject = tmp_path / "subject"
    subject.mkdir()
    ledger = _future_ledger()

    def report():
        if transition == "directory":
            token = input_report.stat_token(subject)
            assert token is not None
            ledger.directory(subject, token)
        else:
            ledger.probe(
                subject / "child", input_report.resolved_probe(subject / "child")
            )

    report()
    report()
    assert ledger.refusals == []
    child = subject / "child"
    child.mkdir()
    # NTFS may defer directory timestamp propagation or coalesce rapid
    # namespace changes. This cell tests the ledger's conflict protocol;
    # explicitly select distinct observable directory generations.
    changed = subject.stat().st_mtime_ns + 5 * 10**9
    if transition == "directory":
        os.utime(subject, ns=(changed, changed))
    report()
    report()
    expected = f"{'directory' if transition == 'directory' else 'lookup'} changed during the build: {subject if transition == 'directory' else child}"
    assert ledger.refusals == [expected]
    child.rmdir()
    if transition == "directory":
        os.utime(subject, ns=(changed + 10**9, changed + 10**9))
    report()
    report()
    assert ledger.refusals == [expected, expected], (
        "restoring the first outcome cannot erase a conflict"
    )


def test_probe_conflicts_keep_prior_refusals_and_distinguish_absence_from_no_fact(
    tmp_path: Path,
) -> None:
    import errno

    subject = tmp_path / "probe"
    ledger = _future_ledger()
    ledger.refuse("prior refusal")
    outcomes = [None, ("", errno.EACCES), ("", errno.ELOOP), None]
    for index, outcome in enumerate(outcomes):
        ledger.probe(subject, outcome)
        ledger.probe(subject, outcome)
        assert (
            ledger.refusals
            == ["prior refusal"]
            + [f"lookup changed during the build: {subject}"] * index
        )
    assert ledger.probes[os.fspath(subject)] is None


def test_an_unreportable_directory_keeps_its_original_fact_and_refusal(
    tmp_path: Path,
) -> None:
    ledger = _future_ledger()
    token = input_report.stat_token(tmp_path)
    assert token is not None
    ledger.directory(tmp_path, token)
    ledger.directory(tmp_path, None)
    assert ledger.directories[os.fspath(tmp_path)] == token
    assert ledger.refusals == [f"unreportable directory {tmp_path}"]


def test_unused_records_are_trimmed(tmp_path: Path) -> None:
    root = tmp_path / "build_records"
    old = build_record.record_path(tmp_path, "a" * 64)
    new = build_record.record_path(tmp_path, "b" * 64)
    for path in (old, new):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("{}", encoding="utf-8")
    _age(old, 8 * 24 * 3600)
    build_record.trim_if_due(root)
    assert not old.exists() and new.exists()
    _age(new, 8 * 24 * 3600)
    build_record.trim_if_due(root)
    assert new.exists(), "trimming runs at most once an hour"


@pytest.mark.usefixtures("fresh_process")
def test_record_trimming_preserves_live_and_unproven_transient_allocations(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    records = tmp_path / "build_records"
    transient = build_record.transient_root(tmp_path)
    abandoned = transient / "build-unproven"
    abandoned.mkdir(parents=True)
    abandoned_payload = abandoned / "worker-output"
    abandoned_payload.write_text("writer custody is unknown", encoding="utf-8")
    old_record = build_record.record_path(tmp_path, "a" * 64)
    old_record.parent.mkdir(parents=True)
    old_record.write_text("{}", encoding="utf-8")
    with build_record.recording(transient):
        import tempfile

        live = Path(tempfile.gettempdir()).parent
        payload = live / "worker-output"
        payload.write_text("live writer", encoding="utf-8")
        # A clock jump makes even the active allocation look expired. The
        # cache record may be retired, but neither scratch has closure proof.
        later = time.time_ns() + 8 * 24 * 3600 * 1_000_000_000
        with monkeypatch.context() as clock:
            clock.setattr(build_record.time, "time_ns", lambda: later)
            build_record.trim_if_due(records)
        assert not old_record.exists()
        assert payload.read_text(encoding="utf-8") == "live writer"
        assert (
            abandoned_payload.read_text(encoding="utf-8") == "writer custody is unknown"
        )
        payload.write_text("writer can continue", encoding="utf-8")
    assert not live.exists(), "scope exit cleans its own completed allocation"
    assert abandoned_payload.read_text(encoding="utf-8") == "writer custody is unknown"


@pytest.mark.usefixtures("fresh_process")
def test_recording_does_not_delete_a_replacement_allocation(
    tmp_path: Path, transient: Path
) -> None:
    moved = tmp_path / "original-allocation"
    with pytest.raises(ValueError, match="temporary directory allocation changed"):
        with build_record.recording(transient):
            import tempfile

            allocated = Path(tempfile.gettempdir()).parent
            allocated.rename(moved)
            allocated.mkdir()
            replacement = allocated / "payload"
            replacement.write_text("another allocation", encoding="utf-8")
    assert replacement.read_text(encoding="utf-8") == "another allocation"
    assert moved.is_dir()


def test_a_hit_refreshes_its_record_for_trimming(tmp_path: Path) -> None:
    _source, _tool, output, record = _fixture(tmp_path)
    _age(record, 2 * 3600)
    aged = record.stat().st_mtime_ns
    assert _current(record, output)
    assert record.stat().st_mtime_ns > aged


def test_the_recorder_stays_outside_the_frontend_fingerprint() -> None:
    # Frontend cache keys commit to the declared producer sources. Readers
    # import only the reporting leaf, which the declaration classifies as
    # reporting, so neither it nor the recorder invalidates lowered modules.
    from molt.cli.cache_fingerprints import (
        _FRONTEND_COMPILER_INPUTS,
        _FrontendInputRole,
        _compiler_root,
        _frontend_semantic_tooling_sources,
    )

    names = {
        path.name for path in _frontend_semantic_tooling_sources(_compiler_root()).paths
    }
    assert _FRONTEND_COMPILER_INPUTS["input_report.py"] is _FrontendInputRole.REPORTING
    assert "build_record.py" not in _FRONTEND_COMPILER_INPUTS
    assert not {"input_report.py", "build_record.py"} & names


def _execute_cached_fixture(source: Path) -> int:
    spec = importlib.util.spec_from_file_location("record_bytecode_fixture", source)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.VALUE


def _worker_execute_cached_fixture(source: Path, prefix: str) -> int:
    sys.pycache_prefix = prefix
    return _execute_cached_fixture(source)


@pytest.mark.parametrize("prefixed", [False, True])
@pytest.mark.parametrize("changed", ["bytecode", "source"])
def test_actual_cached_code_and_source_both_bind_the_record(
    tmp_path: Path, prefixed: bool, changed: str
) -> None:
    _in_fresh_interpreter(_check_cached_code_and_source, tmp_path, prefixed, changed)


def _check_cached_code_and_source(tmp_path: Path, prefixed: bool, changed: str) -> None:
    sys.pycache_prefix = str(tmp_path / "product-cache") if prefixed else None
    source = tmp_path / "subject.py"
    source.write_text("VALUE = 11\n", encoding="utf-8")
    donor = tmp_path / "donor.py"
    donor.write_text("VALUE = 22\n", encoding="utf-8")
    source_stat = source.stat()
    os.utime(donor, ns=(source_stat.st_atime_ns, source_stat.st_mtime_ns))
    cache = Path(importlib.util.cache_from_source(str(source)))
    py_compile.compile(
        str(source),
        cfile=str(cache),
        doraise=True,
        invalidation_mode=py_compile.PycInvalidationMode.TIMESTAMP,
    )
    ledger = _future_ledger()
    input_report.install_sink(ledger, str(tmp_path / "unused-worker-log"))
    try:
        assert _execute_cached_fixture(source) == 11
    finally:
        input_report.remove_sink()
    # The existing product exclusion may cover a shared cache. Actual observed
    # bytecode is an explicit potential input and cannot be discarded with it.
    build_record.resolve_observed(ledger, (cache.parent,))
    ambiguous = os.name == "nt" and prefixed
    assert str(cache) in ledger.files
    assert ("file", str(cache)) in ledger.observed
    if ambiguous:
        assert any("bytecode source drive is ambiguous" in r for r in ledger.refusals)
        assert ("file", str(source)) not in ledger.observed
    else:
        assert str(source) in ledger.files
        assert ("file", str(source)) in ledger.observed
    output = tmp_path / "app"
    output.write_bytes(b"11")
    record, reason = _write(tmp_path, ledger, output)
    if ambiguous:
        assert reason is not None and not record.exists()
    else:
        assert reason is None and _current(record, output)
    if changed == "bytecode":
        # No pyc parser or forged header: equal-size/equal-mtime source files
        # produce caches accepted by ordinary CPython timestamp validation.
        py_compile.compile(
            str(donor),
            cfile=str(cache),
            dfile=str(source),
            doraise=True,
            invalidation_mode=py_compile.PycInvalidationMode.TIMESTAMP,
        )
        assert source.read_text(encoding="utf-8") == "VALUE = 11\n"
        assert _execute_cached_fixture(source) == 22
    else:
        source.write_text("VALUE = 33\n", encoding="utf-8")
    assert not _current(record, output)


def test_worker_reports_source_using_its_own_cache_prefix(
    tmp_path: Path, transient: Path
) -> None:
    _in_fresh_interpreter(_check_worker_cache_prefix, tmp_path, transient)


def _check_worker_cache_prefix(tmp_path: Path, transient: Path) -> None:
    source = tmp_path / "worker_subject.py"
    source.write_text("VALUE = 11\n", encoding="utf-8")
    worker_prefix = str(tmp_path / "worker-cache")
    sys.pycache_prefix = worker_prefix
    cache = Path(importlib.util.cache_from_source(str(source)))
    py_compile.compile(str(source), cfile=str(cache), doraise=True)
    sys.pycache_prefix = str(tmp_path / "parent-cache")
    with build_record.recording(transient) as ledger:
        with ProcessPoolExecutor(
            max_workers=1,
            mp_context=multiprocessing.get_context("spawn"),
            initializer=input_report.worker_initializer,
            initargs=(input_report.worker_log_dir(),),
        ) as pool:
            assert (
                pool.submit(
                    _worker_execute_cached_fixture, source, worker_prefix
                ).result()
                == 11
            )
    assert ("file", str(cache)) in ledger.observed
    assert str(cache) in ledger.files
    if os.name == "nt":
        assert any("bytecode source drive is ambiguous" in r for r in ledger.refusals)
        assert ("file", str(source)) not in ledger.observed
    else:
        assert ("file", str(source)) in ledger.observed
        assert str(source) in ledger.files
        assert not ledger.refusals


@pytest.mark.skipif(os.name != "nt", reason="CPython Windows cache layout")
def test_prefixed_cpython_cache_discards_source_drive(tmp_path):
    _in_fresh_interpreter(_check_prefixed_cache_discards_source_drive, tmp_path)


def _check_prefixed_cache_discards_source_drive(tmp_path):
    sys.pycache_prefix = str(tmp_path / "cache")
    first = importlib.util.cache_from_source(r"C:\same\subject.py")
    second = importlib.util.cache_from_source(r"D:\same\subject.py")
    assert first == second
    source = importlib.util.source_from_cache(first)
    assert not os.path.splitdrive(source)[0]


@pytest.mark.parametrize(
    "filename", ["sourceless.pyc", "malformed.cpython-312.bad-extra.pyc"]
)
def test_unmapped_bytecode_keeps_actual_input_without_guessing_source(
    tmp_path: Path, filename: str
) -> None:
    _in_fresh_interpreter(_check_unmapped_bytecode, tmp_path, filename)


def _check_unmapped_bytecode(tmp_path: Path, filename: str) -> None:
    sys.pycache_prefix = None
    path = tmp_path / filename
    path.write_bytes(b"bytecode")
    ledger = _future_ledger()
    input_report._SINK = ledger
    try:
        input_report._audit_hook("open", (str(path), "r", 0))
    finally:
        input_report._SINK = None
    assert ledger.observed == {("file", str(path))}
    build_record.resolve_observed(ledger, (tmp_path,))
    assert set(ledger.files) == {str(path)}


def test_source_only_record_epoch_is_rejected(tmp_path: Path) -> None:
    _source, _tool, output, record = _fixture(tmp_path)
    payload = json.loads(record.read_text(encoding="utf-8"))
    payload["schema"] = "molt-build-record-v4"
    record.write_text(json.dumps(payload), encoding="utf-8")
    assert build_record.check(record, KEY) == "no record for this request"
