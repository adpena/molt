from __future__ import annotations

import hashlib
import json
import sys
import threading
from pathlib import Path

import pytest

from molt.cli import python_import_resolution, python_source_closure
from tools import profile_python_import_consumer as profiler


def _events(path: Path) -> list[dict]:
    # An abrupt process kill can interrupt the final write; completed records
    # remain usable without pretending that the partial tail is an outcome.
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines(keepends=True)
        if line.endswith("\n")
    ]


def test_progress_is_readable_before_real_binding_returns(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seed = tmp_path / "entry.py"
    content = b"from importlib import import_module as load\nload('payload')\n"
    seed.write_bytes(content)
    (tmp_path / "payload.py").write_text("value = 1\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    events = tmp_path / "events.jsonl"
    summary = tmp_path / "summary.json"
    original = python_import_resolution.analyze_python_bindings
    original_source = python_source_closure.analyze_local_imports
    observed = []

    def inspect_pending_analysis(tree, **kwargs):
        rows = _events(events)
        assert rows[-1]["event"] == "binding_started"
        assert not any(row["event"] == "run_finished" for row in rows)
        started = next(row for row in rows if row["event"] == "source_started")
        assert started["path"] == str(seed)
        assert started["source_sha256"] == hashlib.sha256(content).hexdigest()
        observed.append(kwargs["source_digest"])
        return original(tree, **kwargs)

    monkeypatch.setattr(
        python_import_resolution, "analyze_python_bindings", inspect_pending_analysis
    )

    def run_consumer(args, *, plugins):
        assert args == ["fixture::consumer", "-q"]
        assert len(plugins) == 1
        closure = python_source_closure.local_python_import_closure(tmp_path, (seed,))
        assert {path.name for path in closure.paths} == {"entry.py", "payload.py"}
        return 0

    monkeypatch.setattr(profiler.pytest, "main", run_consumer)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "profiler",
            "fixture::consumer",
            "--min-seconds",
            "0",
            "--events-jsonl",
            str(events),
            "--json",
            str(summary),
        ],
    )
    assert profiler.main() == 0
    rows = _events(events)
    assert observed
    assert rows[0]["event"] == "run_started"
    identity = next(row for row in rows if row["event"] == "identity_finished")
    assert identity["authority_sha256"][str(Path(profiler.__file__).resolve())] == (
        hashlib.sha256(Path(profiler.__file__).read_bytes()).hexdigest()
    )
    assert rows[-1]["event"] == "run_finished"
    assert rows[-1]["exit_code"] == 0
    assert [
        row["ast_digest"] for row in rows if row["event"] == "binding_started"
    ] == observed
    assert python_import_resolution.analyze_python_bindings is inspect_pending_analysis
    assert python_source_closure.analyze_local_imports is original_source
    payload = json.loads(summary.read_text(encoding="utf-8"))
    assert payload["schema_version"] == 2
    assert payload["exit_code"] == 0
    assert payload["slow_analyses"][0]["module"] == "entry"


def test_progress_and_sampler_start_before_authority_capture(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    events = tmp_path / "events.jsonl"
    failure = RuntimeError("identity capture failed")
    original = python_import_resolution.analyze_python_bindings
    original_source = python_source_closure.analyze_local_imports
    sampled = threading.Event()
    emit = profiler._Progress.emit

    def observe(self, event, **fields):
        emit(self, event, **fields)
        if event == "stacks":
            sampled.set()

    def fail_capture():
        assert [row["event"] for row in _events(events) if row["event"] != "stacks"][
            :2
        ] == [
            "run_started",
            "identity_started",
        ]
        assert sampled.wait(5), "sampler was not active during identity capture"
        raise failure

    monkeypatch.setattr(profiler._Progress, "emit", observe)
    monkeypatch.setattr(
        python_import_resolution, "local_import_analysis_identity", fail_capture
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "profiler",
            "fixture",
            "--events-jsonl",
            str(events),
            "--stack-seconds",
            "0.001",
        ],
    )
    with pytest.raises(RuntimeError) as raised:
        profiler.main()
    assert raised.value is failure
    assert [row["event"] for row in _events(events) if row["event"] != "stacks"][
        -1
    ] == "run_failed"
    assert python_import_resolution.analyze_python_bindings is original
    assert python_source_closure.analyze_local_imports is original_source


def test_analysis_failure_preserves_progress_and_propagates(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seed = tmp_path / "entry.py"
    seed.write_text("call()\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    events = tmp_path / "events.jsonl"
    summary = tmp_path / "summary.json"
    failure = RuntimeError("analysis interrupted")

    def fail_analysis(tree, **kwargs):
        assert _events(events)[-1]["event"] == "binding_started"
        raise failure

    monkeypatch.setattr(
        python_import_resolution, "analyze_python_bindings", fail_analysis
    )
    original_source = python_source_closure.analyze_local_imports

    def run_consumer(args, *, plugins):
        python_source_closure.local_python_import_closure(tmp_path, (seed,))

    monkeypatch.setattr(profiler.pytest, "main", run_consumer)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "profiler",
            "fixture",
            "--events-jsonl",
            str(events),
            "--json",
            str(summary),
        ],
    )
    with pytest.raises(RuntimeError) as raised:
        profiler.main()
    assert raised.value is failure
    assert [row["event"] for row in _events(events)][-3:] == [
        "binding_failed",
        "source_failed",
        "run_failed",
    ]
    assert not summary.exists()
    assert python_import_resolution.analyze_python_bindings is fail_analysis
    assert python_source_closure.analyze_local_imports is original_source
    # Closing the stream releases its Windows handle after exceptional exit.
    events.rename(tmp_path / "retained.jsonl")


def test_periodic_stacks_survive_without_a_final_summary(tmp_path: Path) -> None:
    path = tmp_path / "events.jsonl"
    progress = profiler._Progress(path, stack_seconds=0.001)
    received = threading.Event()
    emit = progress.emit

    def observe(event, **fields):
        emit(event, **fields)
        if event == "stacks":
            received.set()

    progress.emit = observe
    try:
        progress.start()
        assert received.wait(5), "stack sampler did not publish"
        rows = _events(path)
        assert any(
            frame["function"] == "test_periodic_stacks_survive_without_a_final_summary"
            for row in rows
            for stack in row["stacks"]
            for frame in stack["frames"]
        )
    finally:
        progress.close()
    assert progress.thread is not None and not progress.thread.is_alive()
    assert all(row["event"] == "stacks" for row in _events(path))


def test_default_invocation_preserves_summary_and_pytest_result(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    summary = tmp_path / "summary.json"
    original = python_import_resolution.analyze_python_bindings
    original_source = python_source_closure.analyze_local_imports

    def run_consumer(args):
        assert args == ["fixture", "-q"]
        assert python_source_closure.analyze_local_imports is original_source
        return pytest.ExitCode.TESTS_FAILED

    monkeypatch.setattr(profiler.pytest, "main", run_consumer)
    monkeypatch.setattr(sys, "argv", ["profiler", "fixture", "--json", str(summary)])
    assert profiler.main() == 1
    payload = json.loads(summary.read_text(encoding="utf-8"))
    assert payload["schema_version"] == 2
    assert payload["exit_code"] == 1
    assert payload["slow_analyses"] == []
    assert python_import_resolution.analyze_python_bindings is original


@pytest.mark.parametrize("same_path", [False, True])
def test_existing_evidence_is_never_overwritten(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    same_path: bool,
) -> None:
    path = tmp_path / "evidence.jsonl"
    path.write_text("previous evidence\n", encoding="utf-8")
    args = ["profiler", "fixture", "--events-jsonl", str(path)]
    if same_path:
        args.extend(["--json", str(path)])
    monkeypatch.setattr(sys, "argv", args)
    with pytest.raises(SystemExit if same_path else FileExistsError):
        profiler.main()
    assert path.read_text(encoding="utf-8") == "previous evidence\n"


@pytest.mark.parametrize("failure_point", ["write", "flush", "sampler", "close"])
@pytest.mark.parametrize("primary_type", [None, RuntimeError, KeyboardInterrupt])
def test_diagnostic_failure_preserves_primary_or_is_fatal_without_one(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure_point: str,
    primary_type: type[BaseException] | None,
) -> None:
    """Real sink failures must not turn an analysis error into an I/O error."""
    events = tmp_path / "events.jsonl"
    summary = tmp_path / "summary.json"
    seed = tmp_path / "entry.py"
    seed.write_text("call()\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    primary = primary_type("original analysis failure") if primary_type else None
    diagnostic = OSError("diagnostic sink unavailable")
    armed = False
    progress_instances = []
    initialize = profiler._Progress.__init__
    original_source = python_source_closure.analyze_local_imports

    class FailingSink:
        def __init__(self, stream):
            self.stream = stream

        def write(self, value):
            if armed and failure_point == "write":
                raise diagnostic
            return self.stream.write(value)

        def flush(self):
            if armed and failure_point == "flush":
                raise diagnostic
            return self.stream.flush()

        def close(self):
            self.stream.close()
            if armed and failure_point == "close":
                raise diagnostic

    def instrument(self, path, *, stack_seconds):
        initialize(self, path, stack_seconds=stack_seconds)
        self.stream = FailingSink(self.stream)
        progress_instances.append(self)

    def fail_analysis(tree, **kwargs):
        nonlocal armed
        assert _events(events)[-1]["event"] == "binding_started"
        armed = True
        if failure_point == "sampler":
            progress_instances[0].error = diagnostic
        assert primary is not None
        raise primary

    def run_consumer(args, *, plugins):
        nonlocal armed
        if primary is not None:
            python_source_closure.local_python_import_closure(tmp_path, (seed,))
        armed = True
        if failure_point == "sampler":
            progress_instances[0].error = diagnostic
        return 0

    monkeypatch.setattr(profiler._Progress, "__init__", instrument)
    monkeypatch.setattr(
        python_import_resolution, "analyze_python_bindings", fail_analysis
    )
    monkeypatch.setattr(profiler.pytest, "main", run_consumer)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "profiler",
            "fixture",
            "--events-jsonl",
            str(events),
            "--json",
            str(summary),
        ],
    )
    with pytest.raises(BaseException) as raised:
        profiler.main()
    assert armed
    assert raised.value is (primary if primary is not None else diagnostic)
    if primary is not None:
        notes = primary.__notes__
        assert any(
            "Profiler diagnostic" in note and "OSError" in note for note in notes
        )
        assert len(notes) <= 4 and all(len(note) < 200 for note in notes)
    assert not summary.exists()
    assert python_import_resolution.analyze_python_bindings is fail_analysis
    assert python_source_closure.analyze_local_imports is original_source
    assert progress_instances[0].stream.stream.closed
    events.rename(tmp_path / "retained.jsonl")
