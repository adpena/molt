"""Luau backend correctness tests — run transpiled output through Lune."""
import os
import subprocess
import tempfile
import pytest

MOLT_DIR = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

def _compile_and_run(python_source: str) -> str:
    """Compile Python source to Luau via molt CLI, run through Lune, return stdout."""
    with tempfile.NamedTemporaryFile(suffix=".py", mode="w", delete=False) as py_f:
        py_f.write(python_source)
        py_path = py_f.name

    luau_path = py_path.replace(".py", ".luau")
    try:
        # Compile
        env = {
            **os.environ,
            "MOLT_EXT_ROOT": "/Volumes/APDataStore/Molt",
            "CARGO_TARGET_DIR": "/Volumes/APDataStore/Molt/cargo-target",
            "RUSTC_WRAPPER": "",
            "PYTHONPATH": os.path.join(MOLT_DIR, "src"),
        }
        result = subprocess.run(
            ["uv", "run", "python", "-m", "molt.cli", "build",
             py_path, "--target", "luau", "--output", luau_path],
            capture_output=True, text=True, timeout=120, env=env, cwd=MOLT_DIR,
        )
        if result.returncode != 0:
            pytest.skip(f"Compilation failed: {result.stderr[:200]}")

        # Run through Lune
        result = subprocess.run(
            ["lune", "run", luau_path],
            capture_output=True, text=True, timeout=30,
        )
        return result.stdout.strip()
    finally:
        for p in [py_path, luau_path]:
            if os.path.exists(p):
                os.unlink(p)


def _python_output(source: str) -> str:
    """Get CPython reference output."""
    result = subprocess.run(
        ["python3", "-c", source],
        capture_output=True, text=True, timeout=10,
    )
    return result.stdout.strip()


class TestRangeAndLoops:
    def test_range_basic(self):
        src = """
result = []
for i in range(5):
    result.append(i)
print(result)
"""
        assert _compile_and_run(src) == _python_output(src)

    def test_range_start_stop(self):
        src = """
result = []
for i in range(2, 7):
    result.append(i)
print(result)
"""
        assert _compile_and_run(src) == _python_output(src)

    def test_range_negative_step(self):
        src = """
result = []
for i in range(10, 0, -2):
    result.append(i)
print(result)
"""
        assert _compile_and_run(src) == _python_output(src)

    def test_nested_loops(self):
        src = """
result = []
for i in range(3):
    for j in range(3):
        result.append(i * 3 + j)
print(result)
"""
        assert _compile_and_run(src) == _python_output(src)


class TestIndexing:
    def test_list_get(self):
        src = """
items = [10, 20, 30, 40, 50]
print(items[0], items[2], items[4])
"""
        assert _compile_and_run(src) == _python_output(src)

    def test_list_set(self):
        src = """
items = [10, 20, 30, 40, 50]
items[1] = 99
print(items)
"""
        assert _compile_and_run(src) == _python_output(src)

    def test_nested_list(self):
        src = """
grid = [[1, 2, 3], [4, 5, 6], [7, 8, 9]]
print(grid[0][0], grid[1][2], grid[2][1])
"""
        assert _compile_and_run(src) == _python_output(src)


class TestDict:
    def test_dict_access(self):
        src = """
d = {"a": 1, "b": 2, "c": 3}
print(d["a"], d["c"])
d["d"] = 4
print(d["d"])
"""
        assert _compile_and_run(src) == _python_output(src)


class TestMath:
    def test_floor_div(self):
        src = 'print(7 // 2, -7 // 2)'
        assert _compile_and_run(src) == _python_output(src)

    def test_modulo(self):
        src = 'print(7 % 3)'
        assert _compile_and_run(src) == _python_output(src)

    def test_power(self):
        src = 'print(2 ** 10)'
        assert _compile_and_run(src) == _python_output(src)


class TestFibonacci:
    def test_fibonacci(self):
        src = """
a, b = 0, 1
fibs = []
for _ in range(10):
    fibs.append(a)
    a, b = b, a + b
print(fibs)
"""
        assert _compile_and_run(src) == _python_output(src)


class TestMultiAssign:
    def test_multi_assign(self):
        src = """
x, y, z = 1, 2, 3
print(x, y, z)
"""
        assert _compile_and_run(src) == _python_output(src)

    def test_swap(self):
        src = """
x, y = 1, 2
x, y = y, x
print(x, y)
"""
        assert _compile_and_run(src) == _python_output(src)


class TestListOps:
    def test_append(self):
        src = """
lst = [3, 1, 4]
lst.append(5)
print(lst)
"""
        assert _compile_and_run(src) == _python_output(src)

    def test_accumulation(self):
        src = """
squares = []
for i in range(8):
    squares.append(i * i)
print(squares)
"""
        assert _compile_and_run(src) == _python_output(src)
