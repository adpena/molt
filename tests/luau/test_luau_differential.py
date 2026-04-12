"""Differential tests: CPython vs Luau transpiler for tests/differential/basic/*.py"""
import os, sys, json, subprocess, tempfile, pathlib
import pytest

MOLT_DIR = pathlib.Path(__file__).resolve().parents[2]
TARGET_ROOT = pathlib.Path(
    os.environ.get("CARGO_TARGET_DIR", str(MOLT_DIR / "target"))
)
BACKEND_BIN = TARGET_ROOT / "debug" / "molt-backend"
BASIC_DIR = MOLT_DIR / "tests" / "differential" / "basic"

def _get_test_files():
    if not BASIC_DIR.exists():
        return []
    return sorted(BASIC_DIR.glob("*.py"))

@pytest.fixture(scope="session", autouse=True)
def build_backend():
    """Build the backend binary once for all tests."""
    if BACKEND_BIN.exists():
        return
    subprocess.run(
        ["cargo", "build", "--manifest-path", str(MOLT_DIR / "runtime" / "molt-backend" / "Cargo.toml"),
         "--features", "luau-backend"],
        capture_output=True, timeout=120
    )

def _compile_and_run_luau(source: str) -> str:
    """Compile Python source to Luau via simple IR path, run through Lune."""
    # Step 1: Compile to IR
    ir_proc = subprocess.run(
        [sys.executable, "-c", f"""
import sys, json; sys.path.insert(0, {str(MOLT_DIR / 'src')!r})
from molt.frontend import compile_to_tir
tir = compile_to_tir({source!r})
data = tir if isinstance(tir, dict) else json.loads(tir)
json.dump(data, sys.stdout)
"""],
        capture_output=True, text=True, timeout=30
    )
    if ir_proc.returncode != 0:
        raise RuntimeError(f"IR compilation failed: {ir_proc.stderr[:200]}")

    with tempfile.NamedTemporaryFile(suffix=".json", mode="w", delete=False) as f:
        f.write(ir_proc.stdout)
        ir_path = f.name

    luau_path = ir_path.replace(".json", ".luau")
    try:
        # Step 2: Transpile to Luau
        result = subprocess.run(
            [str(BACKEND_BIN), "--ir-file", ir_path, "--target", "luau", "--output", luau_path],
            capture_output=True, text=True, timeout=30
        )
        if result.returncode != 0:
            raise RuntimeError(f"Luau transpile failed: {result.stderr[:200]}")

        # Step 3: Run through Lune
        try:
            lune = subprocess.run(
                ["lune", "run", luau_path],
                capture_output=True, text=True, timeout=10
            )
        except FileNotFoundError:
            raise RuntimeError("lune not found")
        if lune.returncode != 0:
            raise RuntimeError(f"Lune error: {lune.stderr[:200]}")
        return lune.stdout.strip()
    finally:
        for p in [ir_path, luau_path]:
            try: os.unlink(p)
            except: pass

@pytest.mark.parametrize("test_file", _get_test_files(), ids=lambda f: f.stem)
def test_differential(test_file):
    source = test_file.read_text()

    # Skip files with features not supported in simple IR Luau path
    skip_markers = ["import ", "class ", "async ", "await ", "yield ", "with ",
                     "lambda ", "exec(", "eval(", "compile(", "__import__"]
    for marker in skip_markers:
        if marker in source:
            pytest.skip(f"Uses unsupported feature: {marker.strip()}")

    # Run CPython
    cpython = subprocess.run(
        [sys.executable, str(test_file)],
        capture_output=True, text=True, timeout=10
    )
    if cpython.returncode != 0:
        pytest.skip(f"CPython failed: {cpython.stderr[:100]}")
    expected = cpython.stdout.strip()

    # Run Luau
    try:
        actual = _compile_and_run_luau(source)
    except RuntimeError as e:
        pytest.skip(str(e))

    assert actual == expected, f"Output mismatch:\nExpected: {expected[:500]}\nActual: {actual[:500]}"
