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


@dataclass
class CompareResult:
    """Result of comparing original vs transformed program output."""

    equivalent: bool
    original_stdout: str
    transformed_stdout: str
    original_stderr: str
    transformed_stderr: str
    error: str | None = None


class MetamorphicRunner:
    """Compile and run two Python sources, comparing output."""

    def __init__(self, build_profile: str = "dev", timeout: int = 30):
        self.build_profile = build_profile
        self.timeout = timeout
        self.python = sys.executable
        self.env = os.environ.copy()
        self.env["PYTHONPATH"] = "src"
        self.env["PYTHONHASHSEED"] = "0"
        self.env["MOLT_DETERMINISTIC"] = "1"

    def _build_and_run(self, source: str, label: str) -> tuple[str, str, str | None]:
        """Build a source string and run it, returning (stdout, stderr, error)."""
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".py", delete=False, prefix=f"metamorphic_{label}_"
        ) as f:
            f.write(source)
            src_path = f.name

        try:
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
            build_result = subprocess.run(
                build_cmd,
                capture_output=True,
                text=True,
                env=self.env,
                timeout=self.timeout,
            )
            if build_result.returncode != 0:
                return "", "", f"Build failed for {label}: {build_result.stderr[:500]}"

            # Extract binary path from JSON output
            import json

            try:
                build_info = json.loads(build_result.stdout)
            except json.JSONDecodeError:
                return "", "", f"Invalid build JSON for {label}"

            binary = None
            for key in ("output", "artifact", "binary", "path", "output_path"):
                if key in build_info:
                    binary = build_info[key]
                    break
            if binary is None and "build" in build_info:
                for key in ("output", "artifact", "binary", "path"):
                    if key in build_info["build"]:
                        binary = build_info["build"][key]
                        break

            if binary is None:
                return "", "", f"Cannot find binary in build output for {label}"

            # Run
            run_result = subprocess.run(
                [binary],
                capture_output=True,
                text=True,
                env=self.env,
                timeout=self.timeout,
            )
            return run_result.stdout, run_result.stderr, None

        except subprocess.TimeoutExpired:
            return "", "", f"Timeout ({self.timeout}s) for {label}"
        finally:
            Path(src_path).unlink(missing_ok=True)

    def compare(self, original: str, transformed: str) -> CompareResult:
        """Compile and run both versions, compare outputs."""
        orig_out, orig_err, orig_error = self._build_and_run(original, "original")
        if orig_error:
            return CompareResult(
                equivalent=False,
                original_stdout=orig_out,
                transformed_stdout="",
                original_stderr=orig_err,
                transformed_stderr="",
                error=orig_error,
            )

        trans_out, trans_err, trans_error = self._build_and_run(
            transformed, "transformed"
        )
        if trans_error:
            return CompareResult(
                equivalent=False,
                original_stdout=orig_out,
                transformed_stdout=trans_out,
                original_stderr=orig_err,
                transformed_stderr=trans_err,
                error=trans_error,
            )

        return CompareResult(
            equivalent=(orig_out == trans_out),
            original_stdout=orig_out,
            transformed_stdout=trans_out,
            original_stderr=orig_err,
            transformed_stderr=trans_err,
        )
