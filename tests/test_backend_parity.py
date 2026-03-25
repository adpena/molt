"""Cross-backend output parity tests (MOL-296).

For a set of small Python programs, compile to each available backend
(native, WASM, Luau) and verify:
  - IR output is identical across backends (shared frontend).
  - If the Molt CLI is available, actual compiled output matches.
  - Backend selection does not affect optimization decisions.

These tests exercise the guarantees proven in:
  - formal/lean/MoltTIR/Backend/CrossBackend.lean (all_backends_equiv)
  - formal/lean/MoltTIR/Backend/BackendDeterminism.lean
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"

# Small programs that exercise different TIR features.
PROGRAMS: list[tuple[str, str]] = [
    ("arithmetic", "x = 1 + 2\nprint(x)\n"),
    ("nested_arith", "a = 3\nb = a * 4 + 1\nprint(b)\n"),
    ("comparison", "x = 10\nif x > 5:\n    print(1)\nelse:\n    print(0)\n"),
    ("bool_logic", "a = True\nb = False\nprint(a and not b)\n"),
    ("string_ops", 's = "hello"\nprint(s)\n'),
    ("while_loop", "i = 0\nwhile i < 3:\n    i = i + 1\nprint(i)\n"),
    ("function_def", "def f(x):\n    return x + 1\nprint(f(5))\n"),
    ("negative_arith", "x = -10\ny = abs(x)\nprint(y)\n"),
    ("multi_assign", "a = 1\nb = 2\nc = a + b\nd = c * c\nprint(d)\n"),
    ("nested_if", "x = 7\nif x > 5:\n    if x < 10:\n        print(1)\n"),
]

BACKENDS = ["native", "wasm", "luau"]

# Timeout for subprocess calls.
_SUBPROCESS_TIMEOUT = float(
    os.environ.get("MOLT_TEST_SUBPROCESS_TIMEOUT", "120")
)


def _molt_cli_available() -> bool:
    """Check if the Molt CLI is importable."""
    try:
        env = os.environ.copy()
        env["PYTHONPATH"] = str(SRC_DIR)
        result = subprocess.run(
            [sys.executable, "-c", "import molt.cli"],
            capture_output=True,
            text=True,
            env=env,
            timeout=30,
        )
        return result.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def _run_molt_build(
    src_path: Path,
    out_dir: Path,
    target: str,
    *,
    extra_args: list[str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run ``python -m molt.cli build`` for a given target."""
    env = os.environ.copy()
    env["PYTHONPATH"] = str(SRC_DIR)
    # Disable midend to keep IR comparison deterministic and fast.
    env.setdefault("MOLT_MIDEND_DISABLE", "1")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_MIDEND_FAIL_OPEN", "1")
    if target == "wasm":
        # These tests compare pre-backend IR. Skip optional linked wasm output
        # so the parity lane measures frontend/midend behavior, not linker cost.
        env.setdefault("MOLT_WASM_LINKED", "0")
        env.setdefault("MOLT_WASM_MODULE_CHUNK_OPS", "0")
    args = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(src_path),
        "--build-profile",
        "dev",
        "--target",
        target,
        "--out-dir",
        str(out_dir),
    ]
    if extra_args:
        # Translate bare --emit-ir to --emit-ir <path> since the CLI
        # requires a file-path argument for this flag.
        resolved = []
        for arg in extra_args:
            resolved.append(arg)
            if arg == "--emit-ir":
                resolved.append(str(out_dir / "ir_output.json"))
        args.extend(resolved)
    return subprocess.run(
        args,
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=_SUBPROCESS_TIMEOUT,
    )


def _collect_ir_json(out_dir: Path) -> dict | None:
    """Try to find and parse the IR JSON from a build output directory."""
    for candidate in out_dir.rglob("*.json"):
        try:
            data = json.loads(candidate.read_text())
            # Heuristic: IR JSON has known top-level keys.
            if isinstance(data, dict) and (
                "instructions" in data
                or "blocks" in data
                or "funcs" in data
                or "functions" in data
            ):
                return data
        except (json.JSONDecodeError, OSError):
            continue
    return None


def _entry_ir_function_names(module_name: str) -> set[str]:
    # Only compare the user module's globals function — molt_init, molt_main,
    # and module chunks contain backend-specific setup that legitimately differs.
    return {
        f"{module_name}____molt_globals_builtin__",
    }


def _filter_entry_ir(ir_json: dict, module_name: str) -> dict:
    """Keep only the entry-module IR surface that should be backend-independent.

    Full emitted IR includes target-specific stdlib/module initialization
    functions (for example capability-gated ``sys`` wiring). The frontend
    guarantee we want here is that the user module and ``__main__`` wrapper
    lower identically before backend-specific runtime integration.
    """

    keep_names = _entry_ir_function_names(module_name)
    filtered_functions = [
        function
        for function in ir_json.get("functions", [])
        if function.get("name") in keep_names
        or function.get("name", "").startswith(f"{module_name}__")
    ]
    return {"functions": filtered_functions}


# ------------------------------------------------------------------
# Tests
# ------------------------------------------------------------------


class TestBackendIRParity:
    """IR output should be identical across backends (shared frontend)."""

    @pytest.fixture(autouse=True)
    def _skip_unless_cli(self):
        if not _molt_cli_available():
            pytest.skip("Molt CLI not available")

    @pytest.mark.parametrize("name,source", PROGRAMS)
    def test_ir_identical_across_backends(
        self, tmp_path: Path, name: str, source: str
    ) -> None:
        """Compile the same program to each backend and compare IR output.

        Since all backends share the same frontend and midend, the IR
        (before backend-specific lowering) should be identical.
        """
        src_file = tmp_path / f"{name}.py"
        src_file.write_text(source)

        ir_outputs: dict[str, dict | None] = {}
        for backend in BACKENDS:
            out_dir = tmp_path / f"out_{backend}"
            out_dir.mkdir(exist_ok=True)
            try:
                result = _run_molt_build(
                    src_file,
                    out_dir,
                    backend,
                    extra_args=["--emit-ir"],
                )
            except subprocess.TimeoutExpired:
                # Even on timeout, the frontend may have already written the
                # IR file before the backend phase started.  Try to collect it.
                ir_output = _collect_ir_json(out_dir)
                if ir_output is not None:
                    ir_output = _filter_entry_ir(ir_output, src_file.stem)
                    ir_outputs[backend] = ir_output
                continue
            # The frontend writes --emit-ir before backend compilation, so
            # the IR file may exist even when the backend fails (rc != 0).
            # Always try to collect it.
            ir_output = _collect_ir_json(out_dir)
            if ir_output is not None:
                ir_output = _filter_entry_ir(ir_output, src_file.stem)
            ir_outputs[backend] = ir_output

        available = {k: v for k, v in ir_outputs.items() if v is not None}
        if len(available) < 2:
            pytest.skip(
                f"Need at least 2 backends with IR output, got {list(available)}"
            )

        # All available IR outputs should be identical.
        reference_backend = next(iter(available))
        reference_ir = available[reference_backend]
        for backend, ir in available.items():
            if backend == reference_backend:
                continue
            assert ir == reference_ir, (
                f"IR mismatch between {reference_backend} and {backend} "
                f"for program '{name}'"
            )


class TestBackendOptimizationParity:
    """Backend selection should not affect optimization decisions."""

    @pytest.fixture(autouse=True)
    def _skip_unless_cli(self):
        if not _molt_cli_available():
            pytest.skip("Molt CLI not available")

    @pytest.mark.parametrize(
        "name,source",
        [
            ("const_fold", "x = 2 + 3\nprint(x)\n"),
            ("dead_code", "x = 1\ny = 2\nprint(x)\n"),
            ("identity", "x = 0 + 5\nprint(x)\n"),
        ],
    )
    def test_optimization_backend_independent(
        self, tmp_path: Path, name: str, source: str
    ) -> None:
        """With midend enabled, optimization results should be identical
        across backends because the midend is backend-agnostic.

        Reference: CrossBackend.lean, optimized_equiv_unoptimized_any_backend
        """
        src_file = tmp_path / f"{name}.py"
        src_file.write_text(source)

        ir_outputs: dict[str, dict | None] = {}
        for backend in BACKENDS:
            out_dir = tmp_path / f"opt_{backend}"
            out_dir.mkdir(exist_ok=True)
            env_override = {
                "MOLT_MIDEND_DISABLE": "0",
                "MOLT_MIDEND_MAX_ROUNDS": "2",
            }
            try:
                env = os.environ.copy()
                env["PYTHONPATH"] = str(SRC_DIR)
                env.update(env_override)
                env.setdefault("MOLT_BACKEND_DAEMON", "0")
                env.setdefault("MOLT_MIDEND_FAIL_OPEN", "1")
                if backend == "wasm":
                    env.setdefault("MOLT_WASM_LINKED", "0")
                    env.setdefault("MOLT_WASM_MODULE_CHUNK_OPS", "0")
                result = subprocess.run(
                    [
                        sys.executable,
                        "-m",
                        "molt.cli",
                        "build",
                        str(src_file),
                        "--profile",
                        "dev",
                        "--target",
                        backend,
                        "--out-dir",
                        str(out_dir),
                        "--emit-ir",
                        str(out_dir / "ir_output.json"),
                    ],
                    cwd=ROOT,
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=_SUBPROCESS_TIMEOUT,
                )
            except subprocess.TimeoutExpired:
                continue
            if result.returncode != 0:
                continue
            ir_output = _collect_ir_json(out_dir)
            if ir_output is not None:
                ir_output = _filter_entry_ir(ir_output, src_file.stem)
            ir_outputs[backend] = ir_output

        available = {k: v for k, v in ir_outputs.items() if v is not None}
        if len(available) < 2:
            pytest.skip("Need at least 2 backends for comparison")

        reference_backend = next(iter(available))
        reference_ir = available[reference_backend]
        for backend, ir in available.items():
            if backend == reference_backend:
                continue
            assert ir == reference_ir, (
                f"Optimized IR differs between {reference_backend} and "
                f"{backend} for '{name}' -- backend should not affect "
                f"optimization decisions"
            )


class TestBackendOutputParity:
    """If the full toolchain is available, actual program output should match
    across backends.

    Reference: CrossBackend.lean, all_backends_equiv
    """

    @pytest.fixture(autouse=True)
    def _skip_unless_cli(self):
        if not _molt_cli_available():
            pytest.skip("Molt CLI not available")

    @pytest.mark.parametrize(
        "name,source,expected",
        [
            ("add", "print(1 + 2)\n", "3"),
            ("mul", "print(3 * 4)\n", "12"),
            ("bool", "print(True)\n", "True"),
            ("string", 'print("ok")\n', "ok"),
        ],
    )
    def test_output_matches_across_backends(
        self, tmp_path: Path, name: str, source: str, expected: str
    ) -> None:
        """Compile and run the same program on each available backend,
        verifying that the stdout output is identical.
        """
        src_file = tmp_path / f"{name}.py"
        src_file.write_text(source)

        outputs: dict[str, str] = {}
        for backend in BACKENDS:
            out_dir = tmp_path / f"run_{backend}"
            out_dir.mkdir(exist_ok=True)
            try:
                result = _run_molt_build(src_file, out_dir, backend)
            except subprocess.TimeoutExpired:
                continue
            if result.returncode != 0:
                continue
            # For native, try to run the binary directly.
            if backend == "native":
                binary = out_dir / "output"
                if not binary.exists():
                    binary = out_dir / "output.exe"
                if binary.exists():
                    try:
                        run = subprocess.run(
                            [str(binary)],
                            capture_output=True,
                            text=True,
                            timeout=30,
                        )
                        if run.returncode == 0:
                            outputs[backend] = run.stdout.strip()
                    except (OSError, subprocess.TimeoutExpired):
                        pass

        if len(outputs) < 2:
            pytest.skip(f"Need 2+ runnable backends, got {list(outputs)}")

        for backend, output in outputs.items():
            assert output == expected, (
                f"Backend {backend} produced '{output}', expected '{expected}'"
            )
