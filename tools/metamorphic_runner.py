#!/usr/bin/env python3
"""Metamorphic testing framework for Molt.

Applies semantics-preserving transformations to Python source programs,
compiles both original and transformed versions, and verifies output
equivalence.

Usage as library:
    from tools.metamorphic_runner import MetamorphicRunner
    runner = MetamorphicRunner()
    result = runner.compare(original_source, transformed_source)
    assert result.equivalent
"""

import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

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
        self.env.setdefault("PYTHONPATH", "src")
        self.env["PYTHONHASHSEED"] = "0"
        self.env["MOLT_DETERMINISTIC"] = "1"
        self.limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", self.env)

    def _build_and_run(
        self,
        source: str,
        label: str,
    ) -> tuple[str, str, int | None, str | None]:
        """Build a source string and run it.

        Returns (stdout, stderr, returncode, error).
        error is None on success.
        """
        src_path = None
        binary_path = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="w",
                suffix=".py",
                delete=False,
                prefix=f"metamorphic_{label}_",
            ) as f:
                f.write(source)
                src_path = f.name

            # Build
            build_cmd = [
                self.python,
                "-m",
                "molt.cli",
                "build",
                "--profile",
                self.build_profile,
                "--deterministic",
                "--json",
                src_path,
            ]
            try:
                build_result = harness_memory_guard.guarded_completed_process(
                    build_cmd,
                    prefix="MOLT_TEST_SUITE",
                    capture_output=True,
                    text=True,
                    env=self.env,
                    timeout=self.build_timeout,
                    limits=self.limits,
                )
            except subprocess.TimeoutExpired:
                return (
                    "",
                    "",
                    None,
                    f"Build timeout ({self.build_timeout:g}s) for {label}",
                )
            if build_result.returncode != 0:
                failure = json_command_failure(build_result.stdout, build_result.stderr)
                return (
                    "",
                    "",
                    build_result.returncode,
                    f"Build failed for {label}: {failure}",
                )
            try:
                binary_path = build_output_path(read_json_payload(build_result.stdout))
            except ValueError as exc:
                return "", "", None, f"Build output for {label}: {exc}"

            if not Path(binary_path).exists():
                return "", "", None, f"Binary not found at {binary_path} for {label}"

            # Run
            run_result = harness_memory_guard.guarded_completed_process(
                [binary_path],
                prefix="MOLT_TEST_SUITE",
                capture_output=True,
                text=True,
                env=self.env,
                timeout=self.timeout,
                limits=self.limits,
            )
            return run_result.stdout, run_result.stderr, run_result.returncode, None

        except subprocess.TimeoutExpired:
            return "", "", None, f"Run timeout ({self.timeout}s) for {label}"
        finally:
            # Clean up source file
            if src_path:
                Path(src_path).unlink(missing_ok=True)
            # Clean up compiled binary
            if binary_path and Path(binary_path).exists():
                Path(binary_path).unlink(missing_ok=True)

    def compare(self, original: str, transformed: str) -> CompareResult:
        """Compile and run both versions, compare outputs."""
        orig_out, orig_err, orig_rc, orig_error = self._build_and_run(
            original,
            "original",
        )
        if orig_error:
            return CompareResult(
                equivalent=False,
                original_stdout=orig_out,
                transformed_stdout="",
                original_stderr=orig_err,
                transformed_stderr="",
                error=orig_error,
                original_rc=orig_rc,
            )

        trans_out, trans_err, trans_rc, trans_error = self._build_and_run(
            transformed,
            "transformed",
        )
        if trans_error:
            return CompareResult(
                equivalent=False,
                original_stdout=orig_out,
                transformed_stdout=trans_out,
                original_stderr=orig_err,
                transformed_stderr=trans_err,
                error=trans_error,
                original_rc=orig_rc,
                transformed_rc=trans_rc,
            )

        return CompareResult(
            equivalent=(orig_out == trans_out),
            original_stdout=orig_out,
            transformed_stdout=trans_out,
            original_stderr=orig_err,
            transformed_stderr=trans_err,
            original_rc=orig_rc,
            transformed_rc=trans_rc,
        )
