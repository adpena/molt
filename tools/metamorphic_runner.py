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

import json
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


def _extract_binary(build_json: dict) -> str | None:
    """Extract the binary path from build JSON, unwrapping data envelope."""
    data = build_json
    if "data" in build_json and isinstance(build_json["data"], dict):
        data = build_json["data"]
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            return data[key]
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                return data["build"][key]
    return None


class MetamorphicRunner:
    """Compile and run two Python sources, comparing output."""

    def __init__(self, build_profile: str = "dev", timeout: int = 30):
        self.build_profile = build_profile
        self.timeout = timeout
        self.python = sys.executable
        self.env = os.environ.copy()
        self.env.setdefault("PYTHONPATH", "src")
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

            try:
                build_info = json.loads(build_result.stdout)
            except json.JSONDecodeError:
                return (
                    "",
                    "",
                    f"Invalid build JSON for {label}: {build_result.stdout[:200]}",
                )

            binary = _extract_binary(build_info)
            if binary is None:
                return (
                    "",
                    "",
                    f"Cannot find binary in build output for {label}. "
                    f"Keys: {list(build_info.keys())}",
                )

            if not Path(binary).exists():
                return "", "", f"Binary not found at {binary} for {label}"

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
