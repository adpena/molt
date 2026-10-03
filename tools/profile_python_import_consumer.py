#!/usr/bin/env python3
"""Profile canonical binding analysis inside one real pytest import consumer.

Optional JSONL progress is flushed before each source/binding analysis, so an
external guard timeout retains unfinished work as well as completed timings.
Periodic Python stacks are diagnostics only; this tool never changes deadlines.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import math
import os
import sys
import threading
import time
from dataclasses import asdict
from pathlib import Path
from typing import Any

import pytest

from molt.cli import python_import_resolution, python_source_closure
from molt.compiler_analysis.python_binding_flow import python_binding_core_computations


def _note_diagnostic_failure(
    primary: BaseException, secondary: BaseException, phase: str
) -> None:
    # Do not invoke user exception __str__ or overridden add_note methods while
    # preserving the original failure. Each of the four unwind sites adds at
    # most one bounded note; even malformed user __notes__ cannot replace it.
    try:
        BaseException.add_note(
            primary,
            f"Profiler diagnostic {phase} also failed ({type(secondary).__name__})."[
                :180
            ],
        )
    except BaseException:
        pass


class _Progress:
    def __init__(self, path: Path, *, stack_seconds: float) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        # Never mix a previous run's completed records with an interrupted run.
        self.stream = path.open("x", encoding="utf-8")
        self.started = time.perf_counter()
        self.lock = threading.Lock()
        self.stop = threading.Event()
        self.error: BaseException | None = None
        self.thread = (
            threading.Thread(target=self._sample, args=(stack_seconds,), daemon=True)
            if stack_seconds
            else None
        )

    def emit(self, event: str, **fields: Any) -> None:
        with self.lock:
            self.stream.write(
                json.dumps(
                    {
                        "schema_version": 1,
                        "event": event,
                        "pid": os.getpid(),
                        "elapsed_seconds": round(time.perf_counter() - self.started, 6),
                        "thread_id": threading.get_ident(),
                        **fields,
                    },
                    sort_keys=True,
                    allow_nan=False,
                )
                + "\n"
            )
            self.stream.flush()

    def _sample(self, interval: float) -> None:
        try:
            while not self.stop.wait(interval):
                stacks = []
                for ident, frame in sys._current_frames().items():
                    if ident == threading.get_ident():
                        continue
                    stack = []
                    while frame is not None:
                        stack.append(
                            {
                                "file": frame.f_code.co_filename,
                                "line": frame.f_lineno,
                                "function": frame.f_code.co_name,
                            }
                        )
                        frame = frame.f_back
                    stacks.append({"thread_id": ident, "frames": stack})
                self.emit("stacks", stacks=stacks)
        except BaseException as exc:
            self.error = exc

    def start(self) -> None:
        if self.thread is not None:
            self.thread.start()

    def close(self) -> None:
        self.stop.set()
        if self.thread is not None and self.thread.ident is not None:
            self.thread.join()
        self.stream.close()
        if self.error is not None:
            raise self.error

    def pytest_runtest_logstart(self, nodeid: str, location: object) -> None:
        self.emit("test_started", nodeid=nodeid)

    def pytest_runtest_logreport(self, report: Any) -> None:
        self.emit(
            "test_phase_finished",
            nodeid=report.nodeid,
            phase=report.when,
            outcome=report.outcome,
            seconds=report.duration,
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pytest_target")
    parser.add_argument("--min-seconds", type=float, default=0.05)
    parser.add_argument("--json", type=Path)
    parser.add_argument("--events-jsonl", type=Path, help="new, flushed progress file")
    parser.add_argument(
        "--stack-seconds",
        type=float,
        default=0,
        help="Python stack sampling interval; requires --events-jsonl (0 disables)",
    )
    args = parser.parse_args()
    if not math.isfinite(args.min_seconds) or args.min_seconds < 0:
        parser.error("--min-seconds must be finite and non-negative")
    if not math.isfinite(args.stack_seconds) or args.stack_seconds < 0:
        parser.error("--stack-seconds must be finite and non-negative")
    if args.stack_seconds and args.events_jsonl is None:
        parser.error("--stack-seconds requires --events-jsonl")
    if args.json is not None and args.events_jsonl is not None:
        if args.json.resolve() == args.events_jsonl.resolve():
            parser.error("summary and progress paths must differ")

    original = python_import_resolution.analyze_python_bindings
    original_source = python_source_closure.analyze_local_imports
    analyses: list[dict[str, Any]] = []
    progress = (
        _Progress(args.events_jsonl, stack_seconds=args.stack_seconds)
        if args.events_jsonl is not None
        else None
    )

    def emit(event: str, **fields: Any) -> None:
        if progress is not None:
            progress.emit(event, **fields)

    def emit_failure(event: str, primary: BaseException, **fields: Any) -> None:
        try:
            emit(event, **fields, error_type=type(primary).__name__)
        except BaseException as secondary:
            _note_diagnostic_failure(primary, secondary, event)

    def measured_source(source: Any, module_source: Any, policy: Any, **kwargs: Any):
        fields = {
            "path": str(source.path),
            "source_sha256": source.sha256,
            "module": module_source.name,
        }
        emit("source_started", **fields)
        started = time.perf_counter()
        try:
            result = original_source(source, module_source, policy, **kwargs)
        except BaseException as exc:
            emit_failure("source_failed", exc, **fields)
            raise
        emit("source_finished", **fields, seconds=time.perf_counter() - started)
        return result

    def measured(tree: ast.Module, **kwargs: Any):
        before = python_binding_core_computations()
        fields = {
            "module": kwargs["policy"].module_name,
            "ast_digest": kwargs["source_digest"],
        }
        emit("binding_started", **fields)
        started = time.perf_counter()
        try:
            index = original(tree, **kwargs)
        except BaseException as exc:
            emit_failure("binding_failed", exc, **fields)
            raise
        elapsed = time.perf_counter() - started
        emit("binding_finished", **fields, seconds=elapsed, states=index.state_count)
        if elapsed >= args.min_seconds:
            policy = kwargs["policy"]
            analyses.append(
                {
                    "module": policy.module_name,
                    "ast_nodes": sum(1 for _node in ast.walk(tree)),
                    "seconds": round(elapsed, 6),
                    "states": index.state_count,
                    "process_core_starts_during_call": (
                        python_binding_core_computations() - before
                    ),
                    "core_fact_telemetry": asdict(index.telemetry),
                }
            )
        return index

    started = time.perf_counter()
    failure: BaseException | None = None
    try:
        if progress is not None:
            emit(
                "run_started",
                pytest_target=args.pytest_target,
                python=sys.executable,
                profiler=str(Path(__file__).absolute()),
                source_closure=str(Path(python_source_closure.__file__).absolute()),
            )
            progress.start()
            emit("identity_started")
            authority_files = (
                Path(__file__).resolve(),
                Path(python_import_resolution.__file__).resolve(),
                Path(python_source_closure.__file__).resolve(),
                Path(sys.modules[original.__module__].__file__).resolve(),
            )
            emit(
                "identity_finished",
                analysis_identity=python_import_resolution.local_import_analysis_identity(),
                authority_sha256={
                    str(path): hashlib.sha256(path.read_bytes()).hexdigest()
                    for path in authority_files
                },
            )
            python_source_closure.analyze_local_imports = measured_source
        python_import_resolution.analyze_python_bindings = measured
        pytest_args = [args.pytest_target, "-q"]
        exit_code = int(
            pytest.main(pytest_args)
            if progress is None
            else pytest.main(pytest_args, plugins=[progress])
        )
        emit("run_finished", exit_code=exit_code)
    except BaseException as exc:
        failure = exc
        emit_failure("run_failed", exc)
        raise
    finally:
        python_import_resolution.analyze_python_bindings = original
        python_source_closure.analyze_local_imports = original_source
        if progress is not None:
            try:
                progress.close()
            except BaseException as secondary:
                if failure is None:
                    raise
                _note_diagnostic_failure(failure, secondary, "cleanup")
    payload = {
        "schema_version": 2,
        "counter_scope": (
            "process-global interval deltas; concurrent calls can overlap; "
            "do not sum per-call rows"
        ),
        "pytest_target": args.pytest_target,
        "wall_seconds": round(time.perf_counter() - started, 6),
        "exit_code": exit_code,
        "slow_analyses": sorted(
            analyses, key=lambda row: float(row["seconds"]), reverse=True
        ),
    }
    encoded = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    if args.json is not None:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
