from __future__ import annotations

import hashlib
import json
import multiprocessing
import os
import stat
import sys
import time
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import pytest

from molt import build_record, input_report
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
    return record, build_record.write_record(record, KEY, ledger, output=output)


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
    return build_record.check(record, key, output) is None


def test_an_unchanged_build_is_current(tmp_path: Path) -> None:
    _source, _tool, output, record = _fixture(tmp_path)
    assert _current(record, output)
    assert not _current(record, output, "j" * 64), "the key binds"


def test_equal_bytes_rewritten_stay_current_only_with_a_digest(tmp_path: Path) -> None:
    source, tool, output, record = _fixture(tmp_path)
    source.write_text("print('a')\n", encoding="utf-8")
    assert _current(record, output), "a touch costs a hash"
    tool.write_text("X = 1\n", encoding="utf-8")
    assert not _current(record, output)


def test_a_content_change_is_stale(tmp_path: Path) -> None:
    source, _tool, output, record = _fixture(tmp_path)
    source.write_text("print('b')\n", encoding="utf-8")
    assert build_record.check(record, KEY, output) == f"file {source}"


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
        sys.audit("subprocess.Popen", "true", ["true"], None, None)
    assert ("file", os.fspath(data)) in ledger.observed
    assert ("directory", os.fspath(tmp_path)) in ledger.observed
    assert ("file", os.fspath(tmp_path / "written.txt")) not in ledger.observed
    assert not any(kind == "written" for kind, _name in ledger.observed)
    assert any(name.endswith(f"{os.sep}true") for name in ledger.files), (
        "the started program is selected through PATH"
    )


@pytest.mark.usefixtures("fresh_process")
def test_a_started_program_records_its_search(tmp_path: Path, transient: Path) -> None:
    # A new tool earlier on PATH, or a retargeted link, selects another program.
    early, late, missing = tmp_path / "early", tmp_path / "late", tmp_path / "none"
    early.mkdir()
    late.mkdir()
    tool = late / "cc"
    tool.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    tool.chmod(0o755)
    search = os.pathsep.join(map(os.fspath, (missing, early, late)))
    with build_record.recording(transient) as ledger:
        input_report._report_program_selection("cc", {"PATH": search}, tmp_path)
    assert ledger.probes[os.fspath(tool)] == (os.path.realpath(tool), stat.S_IFREG)
    assert os.fspath(tool) in ledger.files
    assert os.fspath(early) in ledger.directories, "a tool added here would win"
    assert ledger.probes[os.fspath(missing)] is None, "creating it is a change"
    ledger.started_ns = time.time_ns() + 10 * 10**9
    output = tmp_path / "app"
    output.write_bytes(b"x")
    record, reason = _write(tmp_path, ledger, output)
    assert reason is None and _current(record, output)
    shadow = early / "cc"
    shadow.write_text("#!/bin/sh\n", encoding="utf-8")
    shadow.chmod(0o755)
    assert not _current(record, output)


def test_observations_skip_excluded_roots_and_map_bytecode_to_source(
    tmp_path: Path,
) -> None:
    kept = tmp_path / "src" / "a.py"
    cached = tmp_path / "cache" / "entry"
    bytecode = tmp_path / "src" / "__pycache__" / "a.cpython-312.pyc"
    for path in (kept, cached, bytecode):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")
    ledger = _future_ledger()
    ledger.observed |= {("file", os.fspath(cached)), ("file", os.fspath(bytecode))}
    build_record.resolve_observed(ledger, (tmp_path / "cache",))
    assert set(ledger.files) == {os.fspath(kept)}, "a module loaded from bytecode"


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


def test_only_named_products_are_excluded_and_explicit_inputs_win(
    tmp_path: Path,
) -> None:
    # Existing product custody owns the scratch root and exact published file;
    # its parent and explicitly reported inputs never become implicit products.
    source = tmp_path / "project" / "config.toml"
    scratch = tmp_path / "scratch"
    product = scratch / "object.o"
    output = tmp_path / "out" / "app"
    for path in (source, product, output):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")
    ledger = _future_ledger()
    ledger.observed |= {
        ("file", os.fspath(source)),
        ("file", os.fspath(product)),
        ("file", os.fspath(output)),
        ("directory", os.fspath(source.parent)),
        ("directory", os.fspath(scratch)),
    }
    build_record.resolve_observed(ledger, (scratch,), published_output=output)
    assert set(ledger.files) == {os.fspath(source)}
    assert set(ledger.directories) == {os.fspath(source.parent)}

    explicit = _future_ledger()
    explicit.file(output, input_report.stat_token(output), None)
    explicit.file(product, input_report.stat_token(product), None)
    explicit.observed = ledger.observed.copy()
    build_record.resolve_observed(explicit, (scratch,), published_output=output)
    assert {os.fspath(output), os.fspath(product)} <= explicit.files.keys()


def test_the_outputs_publication_custody_is_a_product(tmp_path: Path) -> None:
    from molt import artifact_publication, file_publication

    output = tmp_path / "out" / "app"
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
    build_record.resolve_observed(ledger, (), published_output=output)
    # Another destination's receipt stays an input; nothing records the
    # directory the publication changed.
    assert set(ledger.files) == {os.fspath(other_receipt)}
    assert ledger.directories == {} and ledger.probes == {}

    # A reader that consumes the directory reports it, and that report stays.
    explicit = _future_ledger()
    explicit.directory(output.parent, input_report.stat_token(output.parent))
    explicit.observed = {("directory", os.fspath(output.parent))}
    build_record.resolve_observed(explicit, (), published_output=output)
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


@pytest.mark.parametrize("payload", [None, False, True, 0, 1.5, "text", [], [1, {}]])
def test_non_object_json_records_miss_without_failing_builds(tmp_path, payload):
    _source, _tool, output, record = _fixture(tmp_path)
    record.write_text(json.dumps(payload), encoding="utf-8")
    assert build_record.check(record, KEY, output) is not None


@pytest.mark.parametrize("field", ["files", "directories", "probes"])
@pytest.mark.parametrize(
    "value", [None, False, 42, "text", [None], [[10**100, [], None]]]
)
def test_malformed_record_collections_miss_without_raising(tmp_path, field, value):
    _source, _tool, output, record = _fixture(tmp_path)
    payload = json.loads(record.read_text(encoding="utf-8"))
    payload[field] = value
    record.write_text(json.dumps(payload), encoding="utf-8")
    assert build_record.check(record, KEY, output) is not None


def test_excessively_nested_json_record_misses(tmp_path):
    _source, _tool, output, record = _fixture(tmp_path)
    record.write_text("[" * 2_000 + "0" + "]" * 2_000, encoding="utf-8")
    assert build_record.check(record, KEY, output) is not None


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
    # Every frontend cache key commits to the sources lowering code imports.
    # Readers import only the reporting leaf, so a change to the recorder never
    # invalidates lowered modules.
    from molt.cli.cache_fingerprints import (
        _compiler_root,
        _frontend_semantic_tooling_sources,
    )

    names = {
        path.name for path in _frontend_semantic_tooling_sources(_compiler_root()).paths
    }
    assert "input_report.py" in names
    assert "build_record.py" not in names
