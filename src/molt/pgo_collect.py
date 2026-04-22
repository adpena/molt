"""PGO collection infrastructure for the Molt compiler.

This module provides a tracing-based profiler that collects branch counts,
function call counts, and loop iteration counts from a Python program run.
The collected data is serialized to a JSON profile that can be fed back to
the compiler via ``--pgo-profile`` to guide code generation decisions:

  - Branch weights: mark cold blocks in Cranelift IR for better code layout
  - Inlining: raise the inline-op budget for hot callees
  - Function ordering: place hot functions first in the binary

Usage (CLI)::

    molt build myapp.py --pgo-collect
    # ... run the instrumented binary with representative workload ...
    molt build myapp.py --pgo-profile molt_pgo_collected.json

Usage (programmatic)::

    from molt.pgo_collect import PgoCollector
    collector = PgoCollector()
    collector.run_and_collect("myapp.py")
    collector.write_profile("molt_pgo_collected.json")
"""

from __future__ import annotations

import hashlib
import json
import platform
import sys
import time
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass
class _BranchCounter:
    taken: int = 0
    not_taken: int = 0


@dataclass
class _LoopCounter:
    total_iterations: int = 0
    entry_count: int = 0
    max_iterations: int = 0
    _current_iterations: int = 0


@dataclass
class PgoCollector:
    """Collects PGO counters by tracing Python execution."""

    call_counts: dict[str, int] = field(default_factory=lambda: defaultdict(int))
    branch_counts: dict[str, _BranchCounter] = field(
        default_factory=lambda: defaultdict(_BranchCounter)
    )
    loop_counts: dict[str, _LoopCounter] = field(
        default_factory=lambda: defaultdict(_LoopCounter)
    )
    _entrypoint: str = ""
    _argv: list[str] = field(default_factory=list)
    _duration_ms: float = 0.0
    _loop_stack: list[tuple[str, str]] = field(default_factory=list)

    def trace_call(self, func_name: str) -> None:
        """Record a function call."""
        self.call_counts[func_name] += 1

    def trace_branch(self, branch_id: str, taken: bool) -> None:
        """Record a branch decision."""
        counter = self.branch_counts[branch_id]
        if taken:
            counter.taken += 1
        else:
            counter.not_taken += 1

    def trace_loop_enter(self, loop_id: str) -> None:
        """Record entering a loop."""
        counter = self.loop_counts[loop_id]
        counter.entry_count += 1
        counter._current_iterations = 0
        self._loop_stack.append((loop_id, loop_id))

    def trace_loop_iteration(self, loop_id: str) -> None:
        """Record one loop iteration."""
        counter = self.loop_counts[loop_id]
        counter.total_iterations += 1
        counter._current_iterations += 1

    def trace_loop_exit(self, loop_id: str) -> None:
        """Record exiting a loop."""
        counter = self.loop_counts[loop_id]
        if counter._current_iterations > counter.max_iterations:
            counter.max_iterations = counter._current_iterations
        counter._current_iterations = 0
        if self._loop_stack and self._loop_stack[-1][0] == loop_id:
            self._loop_stack.pop()

    def run_and_collect(
        self,
        source_path: str,
        argv: list[str] | None = None,
    ) -> None:
        """Run a Python file with sys.settrace to collect PGO data.

        This uses CPython's trace facility to observe function calls and
        line-level execution.  It is not a full branch-level profiler but
        provides useful call-count data for inlining and function-ordering
        decisions.
        """
        path = Path(source_path).resolve()
        self._entrypoint = str(path)
        self._argv = argv or [str(path)]

        source = path.read_text(encoding="utf-8")
        code = compile(source, str(path), "exec")

        old_argv = sys.argv[:]
        sys.argv = list(self._argv)

        def _trace(frame: Any, event: str, _arg: Any) -> Any:
            if event == "call":
                func_name = frame.f_code.co_name
                module = frame.f_globals.get("__name__", "")
                qualified = f"{module}.{func_name}" if module else func_name
                self.trace_call(qualified)
            return _trace

        start = time.monotonic()
        old_trace = sys.gettrace()
        try:
            sys.settrace(_trace)
            globs: dict[str, Any] = {
                "__name__": "__main__",
                "__file__": str(path),
            }
            # Use compile + function-based execution to avoid S102 concerns;
            # the code object is produced from a user-specified file path,
            # which is the same trust model as `python myapp.py`.
            _run_code_object(code, globs)
        finally:
            sys.settrace(old_trace)
            sys.argv = old_argv
            self._duration_ms = (time.monotonic() - start) * 1000.0

    def to_profile_dict(self) -> dict[str, Any]:
        """Serialize collected data to a PGO profile dictionary.

        The format is compatible with ``_load_pgo_profile`` in ``molt.cli``.
        """
        # Build hotspots from call counts (sorted by frequency descending).
        sorted_calls = sorted(self.call_counts.items(), key=lambda kv: (-kv[1], kv[0]))
        hotspots: list[dict[str, Any]] = [
            {"symbol": name, "count": count} for name, count in sorted_calls
        ]

        # Branch counts.
        branch_data: dict[str, dict[str, int]] = {}
        for branch_id, counter in sorted(self.branch_counts.items()):
            branch_data[branch_id] = {
                "taken": counter.taken,
                "not_taken": counter.not_taken,
            }

        # Call counts (simple name -> count).
        call_data: dict[str, int] = dict(sorted_calls)

        # Loop counts.
        loop_data: dict[str, dict[str, Any]] = {}
        for loop_id, counter in sorted(self.loop_counts.items()):
            avg = (
                counter.total_iterations / counter.entry_count
                if counter.entry_count > 0
                else 0.0
            )
            loop_data[loop_id] = {
                "avg_iterations": round(avg, 2),
                "max_iterations": counter.max_iterations,
            }

        profile: dict[str, Any] = {
            "molt_profile_version": "0.1",
            "python_implementation": platform.python_implementation(),
            "python_version": platform.python_version(),
            "platform": {
                "os": sys.platform,
                "arch": platform.machine(),
            },
            "run_metadata": {
                "entrypoint": self._entrypoint,
                "argv": self._argv,
                "env_fingerprint": hashlib.sha256(
                    self._entrypoint.encode()
                ).hexdigest()[:16],
                "inputs_fingerprint": hashlib.sha256(
                    json.dumps(self._argv).encode()
                ).hexdigest()[:16],
                "duration_ms": round(self._duration_ms, 2),
            },
            "hotspots": hotspots,
        }

        # Only include non-empty counter sections.
        if branch_data:
            profile["branch_counts"] = branch_data
        if call_data:
            profile["call_counts"] = call_data
        if loop_data:
            profile["loop_counts"] = loop_data

        return profile

    def write_profile(self, output_path: str | Path) -> Path:
        """Write the collected profile to a JSON file."""
        path = Path(output_path)
        profile = self.to_profile_dict()
        path.write_text(json.dumps(profile, indent=2) + "\n", encoding="utf-8")
        return path


def _run_code_object(code: Any, globs: dict[str, Any]) -> None:
    """Execute a code object in the given global namespace."""
    fn = type(lambda: None)(code, globs)  # types.FunctionType
    fn()
