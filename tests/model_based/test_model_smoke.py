"""Smoke tests for model-based test generation from Quint traces.

Verifies that each Quint model can produce traces and that the generated
Python test programs are syntactically valid (compile without error).

Usage::

    uv run --python 3.12 python3 -m pytest tests/model_based/test_model_smoke.py -v
"""

from __future__ import annotations

import ast
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

# Repository root — two levels up from this file
_REPO_ROOT = Path(__file__).resolve().parents[2]
_QUINT_DIR = _REPO_ROOT / "formal" / "quint"

# Models to test — maps model stem to its invariant name
_MODELS: dict[str, str] = {
    "molt_build_determinism": "Inv",
    "molt_runtime_determinism": "Inv",
    "molt_midend_pipeline": "inv",
    "molt_calling_convention": "Inv",
    # molt_cross_version excluded: has a known Quint parse error (QNT404)
    "molt_luau_transpiler": "Inv",
}


def _quint_available() -> bool:
    """Check if quint CLI is available."""
    return shutil.which("quint") is not None


def _model_path(stem: str) -> Path:
    return _QUINT_DIR / f"{stem}.qnt"


@pytest.fixture(scope="module")
def tmp_output_dir(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return tmp_path_factory.mktemp("mbt_smoke")


def _skip_if_no_quint() -> None:
    if not _quint_available():
        pytest.skip("quint CLI not found")


def _run_generator(
    model_stem: str,
    output_dir: Path,
    *,
    max_steps: int = 8,
    count: int = 2,
) -> list[Path]:
    """Run the generator and return paths of generated test files."""
    model = _model_path(model_stem)
    if not model.exists():
        pytest.skip(f"Model not found: {model}")

    tool_path = _REPO_ROOT / "tools" / "quint_trace_to_tests.py"
    result = subprocess.run(
        [
            sys.executable,
            str(tool_path),
            "--model",
            str(model),
            "--max-steps",
            str(max_steps),
            "--count",
            str(count),
            "--output-dir",
            str(output_dir),
        ],
        capture_output=True,
        text=True,
        timeout=120,
        cwd=str(_REPO_ROOT),
    )

    if result.returncode != 0:
        pytest.fail(
            f"Generator failed for {model_stem}:\n"
            f"  stdout: {result.stdout}\n"
            f"  stderr: {result.stderr}"
        )

    # Collect generated .py files
    files = sorted(output_dir.glob(f"mbt_{model_stem.replace('molt_', '')}*.py"))
    return files


@pytest.mark.parametrize("model_stem", list(_MODELS.keys()))
def test_generates_valid_python(
    model_stem: str,
    tmp_output_dir: Path,
) -> None:
    """Each model generates syntactically valid Python programs."""
    _skip_if_no_quint()

    model_dir = tmp_output_dir / model_stem
    model_dir.mkdir(exist_ok=True)

    files = _run_generator(model_stem, model_dir, count=2)
    assert len(files) > 0, f"No test files generated for {model_stem}"

    for f in files:
        source = f.read_text(encoding="utf-8")
        # Must be valid Python (no syntax errors)
        try:
            ast.parse(source, filename=str(f))
        except SyntaxError as e:
            pytest.fail(
                f"Generated file {f.name} has syntax error: {e}\nSource:\n{source}"
            )


@pytest.mark.parametrize("model_stem", list(_MODELS.keys()))
def test_generated_programs_run(
    model_stem: str,
    tmp_output_dir: Path,
) -> None:
    """Each generated program runs without error under CPython."""
    _skip_if_no_quint()

    model_dir = tmp_output_dir / model_stem
    model_dir.mkdir(exist_ok=True)

    files = _run_generator(model_stem, model_dir, count=2)
    assert len(files) > 0

    for f in files:
        result = subprocess.run(
            [sys.executable, str(f)],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode != 0:
            pytest.fail(
                f"Generated program {f.name} failed:\n"
                f"  stdout: {result.stdout}\n"
                f"  stderr: {result.stderr}"
            )


@pytest.mark.parametrize("model_stem", list(_MODELS.keys()))
def test_generated_programs_have_docstrings(
    model_stem: str,
    tmp_output_dir: Path,
) -> None:
    """Each generated program has a Purpose docstring."""
    _skip_if_no_quint()

    model_dir = tmp_output_dir / model_stem
    model_dir.mkdir(exist_ok=True)

    files = _run_generator(model_stem, model_dir, count=1)
    assert len(files) > 0

    for f in files:
        source = f.read_text(encoding="utf-8")
        tree = ast.parse(source)
        docstring = ast.get_docstring(tree)
        assert docstring is not None, f"No docstring in {f.name}"
        assert "Purpose:" in docstring, f"Docstring missing 'Purpose:' in {f.name}"


def test_json_report(tmp_output_dir: Path) -> None:
    """The --json flag produces valid JSON output."""
    _skip_if_no_quint()

    import json

    model = _model_path("molt_build_determinism")
    if not model.exists():
        pytest.skip("Model not found")

    tool_path = _REPO_ROOT / "tools" / "quint_trace_to_tests.py"
    json_dir = tmp_output_dir / "json_test"
    json_dir.mkdir(exist_ok=True)

    result = subprocess.run(
        [
            sys.executable,
            str(tool_path),
            "--model",
            str(model),
            "--max-steps",
            "5",
            "--count",
            "1",
            "--output-dir",
            str(json_dir),
            "--json",
        ],
        capture_output=True,
        text=True,
        timeout=120,
        cwd=str(_REPO_ROOT),
    )

    assert result.returncode == 0, f"stderr: {result.stderr}"
    report = json.loads(result.stdout)
    assert "model" in report
    assert "count_generated" in report
    assert "tests" in report
    assert isinstance(report["tests"], list)
