#!/usr/bin/env python3
"""Metamorphic testing framework for Molt.

Applies semantics-preserving transformations to Python source programs,
compiles both original and transformed versions, and verifies output
equivalence.

CPython is the independent oracle. It runs both sources first: different
CPython outputs mean the transformation is not semantics-preserving, which is
a defect in the test. Each compiled program must then exit 0 and print what
CPython printed, so two compiled programs that fail the same way never count
as equivalent.

Usage as library:
    from tools.metamorphic_runner import MetamorphicRunner
    runner = MetamorphicRunner()
    result = runner.compare(original_source, transformed_source)
    assert result.equivalent, result.describe()
"""

import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


from molt.cargo_execution_policy import default_nested_process_timeout_seconds  # noqa: E402
from molt.cli.output import (  # noqa: E402
    build_output_path,
    json_command_failure,
    read_json_payload,
)
from tools import harness_memory_guard  # noqa: E402


@dataclass
class CompareResult:
    """Result of comparing original vs transformed program output."""

    equivalent: bool
    original_stdout: str
    transformed_stdout: str
    original_stderr: str
    transformed_stderr: str
    error: str | None = None
    original_rc: int | None = None
    transformed_rc: int | None = None
    cpython_stdout: str = ""
    cpython_rc: int | None = None

    def describe(self) -> str:
        return (
            f"CPython stdout:     {self.cpython_stdout!r} (rc={self.cpython_rc})\n"
            f"Original stdout:    {self.original_stdout!r} (rc={self.original_rc})\n"
            f"Transformed stdout: {self.transformed_stdout!r} "
            f"(rc={self.transformed_rc})\n"
            f"Original stderr:    {self.original_stderr[-2000:]!r}\n"
            f"Transformed stderr: {self.transformed_stderr[-2000:]!r}"
        )


@dataclass(frozen=True)
class _Run:
    stdout: str
    stderr: str
    returncode: int | None
    error: str | None = None


class MetamorphicRunner:
    """Compile and run two Python sources, comparing output."""

    def __init__(
        self,
        build_profile: str = "dev",
        timeout: int = 30,
        build_timeout: float | None = None,
    ):
        """``timeout`` bounds each program run; a build gets the proof plan's
        nested build budget unless ``build_timeout`` names one."""
        self.build_profile = build_profile
        self.timeout = timeout
        self.build_timeout = (
            default_nested_process_timeout_seconds("build")
            if build_timeout is None
            else build_timeout
        )
        self.python = sys.executable
        self.env = os.environ.copy()
        self.env.setdefault("PYTHONPATH", str(REPO_ROOT / "src"))
        self.env["PYTHONHASHSEED"] = "0"
        self.env["MOLT_DETERMINISTIC"] = "1"
        self.limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", self.env)

    def _run(self, argv: list[str], timeout: float) -> subprocess.CompletedProcess[str]:
        return harness_memory_guard.guarded_completed_process(
            argv,
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            env=self.env,
            timeout=timeout,
            limits=self.limits,
            cwd=REPO_ROOT,
        )

    def _run_cpython(self, source_path: Path, label: str) -> _Run:
        try:
            result = self._run([self.python, str(source_path)], self.timeout)
        except subprocess.TimeoutExpired:
            return _Run(
                "", "", None, f"CPython run timeout ({self.timeout}s) for {label}"
            )
        return _Run(result.stdout, result.stderr, result.returncode)

    def _build_and_run(self, source_path: Path, out_dir: Path, label: str) -> _Run:
        """Build one source with Molt and run the binary."""
        build_cmd = [
            self.python,
            "-m",
            "molt.cli",
            "build",
            "--profile",
            self.build_profile,
            "--deterministic",
            "--json",
            "--out-dir",
            str(out_dir),
            "--output",
            str(out_dir / label),
            str(source_path),
        ]
        try:
            build_result = self._run(build_cmd, self.build_timeout)
        except subprocess.TimeoutExpired:
            return _Run(
                "", "", None, f"Build timeout ({self.build_timeout:g}s) for {label}"
            )
        if build_result.returncode != 0:
            failure = json_command_failure(build_result.stdout, build_result.stderr)
            return _Run(
                "",
                "",
                build_result.returncode,
                f"Build failed for {label}: {failure}",
            )
        try:
            binary_path = build_output_path(read_json_payload(build_result.stdout))
        except ValueError as exc:
            return _Run("", "", None, f"Build output for {label}: {exc}")
        if not Path(binary_path).exists():
            return _Run("", "", None, f"Binary not found at {binary_path} for {label}")
        try:
            run_result = self._run([binary_path], self.timeout)
        except subprocess.TimeoutExpired:
            return _Run("", "", None, f"Run timeout ({self.timeout}s) for {label}")
        return _Run(run_result.stdout, run_result.stderr, run_result.returncode)

    def compare(self, original: str, transformed: str) -> CompareResult:
        """Run both versions under CPython, then compile, run and compare them."""
        # Each operation owns a private directory. A build reads its entry
        # script's directory as an import root, so a shared temporary
        # directory would let unrelated files enter that root mid-build.
        # Binaries and build intermediates go to a sibling directory, outside
        # the import root: without --out-dir, native builds keep intermediates
        # in <MOLT_HOME>/build/<stem>, which concurrent builds of different
        # programs with one stem share.
        with tempfile.TemporaryDirectory(prefix="metamorphic_") as scratch:
            root = Path(scratch)
            source_dir = root / "src"
            out_dir = root / "out"
            source_dir.mkdir()
            out_dir.mkdir()
            sources = {"original": original, "transformed": transformed}
            paths = {}
            for label, text in sources.items():
                paths[label] = source_dir / f"{label}.py"
                paths[label].write_text(text, encoding="utf-8")

            cpython = {
                label: self._run_cpython(path, label) for label, path in paths.items()
            }
            reference = cpython["original"]
            error = reference.error or cpython["transformed"].error
            if error is None and reference.returncode != 0:
                error = f"CPython failed on the original: {reference.stderr[-2000:]}"
            if error is None and (
                cpython["transformed"].stdout != reference.stdout
                or cpython["transformed"].returncode != reference.returncode
            ):
                error = (
                    "the transformation changes CPython's output, so it is not "
                    f"semantics-preserving: {reference.stdout!r} != "
                    f"{cpython['transformed'].stdout!r}"
                )
            molt = {}
            if error is None:
                for label, path in paths.items():
                    molt[label] = self._build_and_run(path, out_dir, label)
                    if molt[label].error is not None:
                        error = molt[label].error
                        break
        empty = _Run("", "", None)
        orig = molt.get("original", empty)
        trans = molt.get("transformed", empty)
        return CompareResult(
            equivalent=(
                error is None
                and orig.returncode == trans.returncode == reference.returncode
                and orig.stdout == trans.stdout == reference.stdout
            ),
            original_stdout=orig.stdout,
            transformed_stdout=trans.stdout,
            original_stderr=orig.stderr,
            transformed_stderr=trans.stderr,
            error=error,
            original_rc=orig.returncode,
            transformed_rc=trans.returncode,
            cpython_stdout=reference.stdout,
            cpython_rc=reference.returncode,
        )
