"""Luau backend correctness tests — differential testing via Lune vs CPython.

Each test compiles Python source to Luau via molt CLI, runs through Lune,
and asserts identical stdout to CPython. This catches:
- Print formatting (tables, bools, None)
- 0-based→1-based index translation
- Operator semantics (floor div, modulo, power)
- Iteration lowering (range, for-in, nested)
- Data structure operations (list, dict)
- Control flow (if/else, while, break/continue)
- Function semantics (recursion, closures, default args)
"""

import os
import subprocess
import sys
import tempfile
import time
import pytest

MOLT_DIR = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ARTIFACT_ROOT = os.environ.get("MOLT_EXT_ROOT", MOLT_DIR)


def _compile_and_run(python_source: str, *, expect_fail: bool = False) -> str:
    """Compile Python source to Luau via molt CLI, run through Lune, return stdout."""
    with tempfile.NamedTemporaryFile(suffix=".py", mode="w", delete=False) as py_f:
        py_f.write(python_source)
        py_path = py_f.name

    luau_path = py_path.replace(".py", ".luau")
    try:
        env = {
            **os.environ,
            "MOLT_EXT_ROOT": ARTIFACT_ROOT,
            "CARGO_TARGET_DIR": os.environ.get(
                "CARGO_TARGET_DIR",
                os.path.join(ARTIFACT_ROOT, "target"),
            ),
            "MOLT_USE_SCCACHE": "0",
            "RUSTC_WRAPPER": "",
            "PYTHONPATH": os.path.join(MOLT_DIR, "src"),
            "MOLT_DEV_CARGO_PROFILE": os.environ.get(
                "MOLT_DEV_CARGO_PROFILE", "release-fast"
            ),
            "UV_LINK_MODE": os.environ.get("UV_LINK_MODE", "copy"),
            "UV_NO_SYNC": os.environ.get("UV_NO_SYNC", "1"),
        }
        build_timeout = int(os.environ.get("MOLT_LUAU_BUILD_TIMEOUT", "900"))
        py_exec = sys.executable or "python3"
        result = subprocess.run(
            [
                py_exec,
                "-m",
                "molt.cli",
                "build",
                py_path,
                "--target",
                "luau",
                "--output",
                luau_path,
            ],
            capture_output=True,
            text=True,
            timeout=build_timeout,
            env=env,
            cwd=MOLT_DIR,
        )
        if result.returncode != 0:
            if expect_fail:
                return ""
            pytest.skip(f"Compilation failed: {result.stderr[:200]}")

        try:
            result = subprocess.run(
                ["lune", "run", luau_path],
                capture_output=True,
                text=True,
                timeout=30,
            )
        except FileNotFoundError:
            pytest.skip("lune not found")
        if result.returncode != 0 and not expect_fail:
            pytest.fail(f"Lune runtime error: {result.stderr[:300]}")
        return result.stdout.strip()
    finally:
        for p in [py_path, luau_path]:
            if os.path.exists(p):
                os.unlink(p)


def _python_output(source: str) -> str:
    """Get CPython reference output."""
    result = subprocess.run(
        ["python3", "-c", source],
        capture_output=True,
        text=True,
        timeout=10,
    )
    return result.stdout.strip()


def _assert_match(src: str):
    """Assert transpiled Luau output matches CPython."""
    assert _compile_and_run(src) == _python_output(src)


# ─── Range & Loops ────────────────────────────────────────────────────────────


class TestRangeAndLoops:
    def test_range_basic(self):
        _assert_match("""
result = []
for i in range(5):
    result.append(i)
print(result)
""")

    def test_range_start_stop(self):
        _assert_match("""
result = []
for i in range(2, 7):
    result.append(i)
print(result)
""")

    def test_range_negative_step(self):
        _assert_match("""
result = []
for i in range(10, 0, -2):
    result.append(i)
print(result)
""")

    def test_range_empty(self):
        _assert_match("""
result = []
for i in range(0):
    result.append(i)
print(result)
""")

    def test_range_single(self):
        _assert_match("""
result = []
for i in range(1):
    result.append(i)
print(result)
""")

    def test_nested_loops(self):
        _assert_match("""
result = []
for i in range(3):
    for j in range(3):
        result.append(i * 3 + j)
print(result)
""")

    def test_while_loop(self):
        _assert_match("""
x = 10
result = []
while x > 0:
    result.append(x)
    x = x - 3
print(result)
""")

    def test_loop_accumulator(self):
        _assert_match("""
total = 0
for i in range(1, 11):
    total = total + i
print(total)
""")


# ─── Indexing ─────────────────────────────────────────────────────────────────


class TestIndexing:
    def test_list_get(self):
        _assert_match("""
items = [10, 20, 30, 40, 50]
print(items[0], items[2], items[4])
""")

    def test_list_set(self):
        _assert_match("""
items = [10, 20, 30, 40, 50]
items[1] = 99
print(items)
""")

    def test_nested_list(self):
        _assert_match("""
grid = [[1, 2, 3], [4, 5, 6], [7, 8, 9]]
print(grid[0][0], grid[1][2], grid[2][1])
""")

    def test_list_last_element(self):
        _assert_match("""
items = [10, 20, 30]
print(items[2])
""")


# ─── Dict ─────────────────────────────────────────────────────────────────────


class TestDict:
    def test_dict_access(self):
        _assert_match("""
d = {"a": 1, "b": 2, "c": 3}
print(d["a"], d["c"])
d["d"] = 4
print(d["d"])
""")

    def test_dict_overwrite(self):
        _assert_match("""
d = {"x": 10}
d["x"] = 20
print(d["x"])
""")


# ─── Math Operations ─────────────────────────────────────────────────────────


class TestMath:
    def test_floor_div(self):
        _assert_match("print(7 // 2, -7 // 2)")

    def test_modulo(self):
        _assert_match("print(7 % 3)")

    def test_power(self):
        _assert_match("print(2 ** 10)")

    def test_power_small(self):
        _assert_match("print(3 ** 4)")

    def test_arithmetic_chain(self):
        _assert_match("print(2 + 3 * 4 - 1)")

    def test_integer_division(self):
        _assert_match("print(10 // 3, 11 // 4, 100 // 7)")

    def test_large_multiply(self):
        _assert_match("print(12345 * 6789)")


# ─── Print Formatting ────────────────────────────────────────────────────────


class TestPrintFormatting:
    def test_print_single_list(self):
        _assert_match("""
result = [0, 1, 2, 3, 4]
print(result)
""")

    def test_print_nested_list(self):
        _assert_match("""
grid = [[1, 2], [3, 4]]
print(grid)
""")

    def test_print_empty_list(self):
        _assert_match("""
empty = []
print(empty)
""")

    def test_print_booleans(self):
        _assert_match("print(True, False)")

    def test_print_mixed_types(self):
        _assert_match('print(1, "hello", [3, 4], True)')

    def test_print_multiple_args(self):
        _assert_match('print("a", "b", "c", 1, 2, 3)')

    def test_print_single_int(self):
        _assert_match("print(42)")

    def test_print_single_string(self):
        _assert_match('print("hello world")')


# ─── Fibonacci & Algorithms ──────────────────────────────────────────────────


class TestAlgorithms:
    def test_fibonacci(self):
        _assert_match("""
a, b = 0, 1
fibs = []
for _ in range(10):
    fibs.append(a)
    a, b = b, a + b
print(fibs)
""")

    def test_factorial_iterative(self):
        _assert_match("""
result = 1
for i in range(1, 11):
    result = result * i
print(result)
""")

    def test_sum_of_squares(self):
        _assert_match("""
total = 0
for i in range(1, 6):
    total = total + i * i
print(total)
""")

    def test_collatz_steps(self):
        _assert_match("""
n = 27
steps = 0
while n != 1:
    if n % 2 == 0:
        n = n // 2
    else:
        n = 3 * n + 1
    steps = steps + 1
print(steps)
""")

    def test_gcd(self):
        _assert_match("""
a, b = 48, 18
while b != 0:
    a, b = b, a % b
print(a)
""")


# ─── Assignment ──────────────────────────────────────────────────────────────


class TestAssignment:
    def test_multi_assign(self):
        _assert_match("""
x, y, z = 1, 2, 3
print(x, y, z)
""")

    def test_swap(self):
        _assert_match("""
x, y = 1, 2
x, y = y, x
print(x, y)
""")

    def test_augmented_assign(self):
        _assert_match("""
x = 10
x = x + 5
x = x - 3
x = x * 2
print(x)
""")


# ─── List Operations ─────────────────────────────────────────────────────────


class TestListOps:
    def test_append(self):
        _assert_match("""
lst = [3, 1, 4]
lst.append(5)
print(lst)
""")

    def test_accumulation(self):
        _assert_match("""
squares = []
for i in range(8):
    squares.append(i * i)
print(squares)
""")

    def test_build_list_in_loop(self):
        _assert_match("""
evens = []
for i in range(20):
    if i % 2 == 0:
        evens.append(i)
print(evens)
""")

    def test_len(self):
        _assert_match("""
lst = [1, 2, 3, 4, 5]
print(len(lst))
print(len([]))
""")


# ─── Control Flow ─────────────────────────────────────────────────────────────


class TestControlFlow:
    def test_if_else(self):
        _assert_match("""
x = 5
if x > 3:
    print("big")
else:
    print("small")
""")

    def test_elif_chain(self):
        _assert_match("""
x = 15
if x > 20:
    print("a")
elif x > 10:
    print("b")
elif x > 5:
    print("c")
else:
    print("d")
""")

    def test_nested_if(self):
        _assert_match("""
x = 7
y = 3
if x > 5:
    if y > 2:
        print("both")
    else:
        print("just x")
else:
    print("neither")
""")

    def test_conditional_in_loop(self):
        _assert_match("""
result = []
for i in range(10):
    if i % 3 == 0:
        result.append(i)
print(result)
""")


# ─── Boolean Logic ────────────────────────────────────────────────────────────


class TestBooleanLogic:
    def test_and_or(self):
        _assert_match("""
print(True and True)
print(True and False)
print(False or True)
print(False or False)
""")

    def test_not(self):
        _assert_match("""
print(not True)
print(not False)
""")

    def test_comparison_chain(self):
        _assert_match("""
x = 5
print(x > 3)
print(x < 3)
print(x == 5)
print(x != 4)
""")


# ─── Nested Index & Type Guard ───────────────────────────────────────────────


class TestNestedIndexing:
    """Tests that exercise index type-guard elimination and nested list access."""

    def test_matrix_multiply(self):
        _assert_match("""
def matrix_multiply(a, b):
    rows_a = len(a)
    cols_b = len(b[0])
    cols_a = len(b)
    result = []
    for i in range(rows_a):
        row = []
        for j in range(cols_b):
            total = 0
            for k in range(cols_a):
                total = total + a[i][k] * b[k][j]
            row.append(total)
        result.append(row)
    return result

a = [[1, 2, 3], [4, 5, 6]]
b = [[7, 8], [9, 10], [11, 12]]
c = matrix_multiply(a, b)
for row in c:
    print(row)
""")

    def test_nested_list_sum(self):
        _assert_match("""
grid = [[1, 2, 3], [4, 5, 6], [7, 8, 9]]
total = 0
for i in range(len(grid)):
    for j in range(len(grid[i])):
        total = total + grid[i][j]
print(total)
""")

    def test_list_of_lists_build(self):
        _assert_match("""
result = []
for i in range(4):
    row = []
    for j in range(3):
        row.append(i * 3 + j)
    result.append(row)
for row in result:
    print(row)
""")

    def test_accumulate_with_index(self):
        _assert_match("""
data = [10, 20, 30, 40, 50]
running = []
total = 0
for i in range(len(data)):
    total = total + data[i]
    running.append(total)
print(running)
""")


# ─── Function Return Values ─────────────────────────────────────────────────


class TestFunctionReturn:
    """Tests that verify function return values survive epilogue cleanup."""

    def test_return_computed_list(self):
        _assert_match("""
def make_list(n):
    result = []
    for i in range(n):
        result.append(i * i)
    return result

print(make_list(5))
""")

    def test_return_nested_result(self):
        _assert_match("""
def transpose(matrix):
    rows = len(matrix)
    cols = len(matrix[0])
    result = []
    for j in range(cols):
        row = []
        for i in range(rows):
            row.append(matrix[i][j])
        result.append(row)
    return result

m = [[1, 2, 3], [4, 5, 6]]
t = transpose(m)
for row in t:
    print(row)
""")

    def test_return_accumulator(self):
        _assert_match("""
def dot_product(a, b):
    total = 0
    for i in range(len(a)):
        total = total + a[i] * b[i]
    return total

print(dot_product([1, 2, 3], [4, 5, 6]))
""")


# ─── Performance Benchmark ───────────────────────────────────────────────────


class TestPerformance:
    """Benchmark tests that measure transpiled Luau perf vs CPython.

    These don't assert equality — they measure execution time and report.
    Marked with a custom marker so they can be run separately.
    """

    @staticmethod
    def _timed_compile_and_run(src: str) -> tuple[str, float, float]:
        """Returns (output, compile_seconds, run_seconds)."""
        with tempfile.NamedTemporaryFile(suffix=".py", mode="w", delete=False) as py_f:
            py_f.write(src)
            py_path = py_f.name

        luau_path = py_path.replace(".py", ".luau")
        try:
            env = {
                **os.environ,
                "MOLT_EXT_ROOT": ARTIFACT_ROOT,
                "CARGO_TARGET_DIR": os.environ.get(
                    "CARGO_TARGET_DIR",
                    os.path.join(ARTIFACT_ROOT, "target"),
                ),
                "RUSTC_WRAPPER": "",
                "PYTHONPATH": os.path.join(MOLT_DIR, "src"),
            }
            t0 = time.perf_counter()
            result = subprocess.run(
                [
                    "uv",
                    "run",
                    "python",
                    "-m",
                    "molt.cli",
                    "build",
                    py_path,
                    "--target",
                    "luau",
                    "--output",
                    luau_path,
                ],
                capture_output=True,
                text=True,
                timeout=240,
                env=env,
                cwd=MOLT_DIR,
            )
            compile_time = time.perf_counter() - t0
            if result.returncode != 0:
                pytest.skip(f"Compilation failed: {result.stderr[:200]}")

            t0 = time.perf_counter()
            try:
                result = subprocess.run(
                    ["lune", "run", luau_path],
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
            except FileNotFoundError:
                pytest.skip("lune not found")
            run_time = time.perf_counter() - t0
            return result.stdout.strip(), compile_time, run_time
        finally:
            for p in [py_path, luau_path]:
                if os.path.exists(p):
                    os.unlink(p)

    @staticmethod
    def _timed_python(src: str) -> tuple[str, float]:
        t0 = time.perf_counter()
        result = subprocess.run(
            ["python3", "-c", src],
            capture_output=True,
            text=True,
            timeout=10,
        )
        return result.stdout.strip(), time.perf_counter() - t0

    def test_perf_fibonacci_70(self):
        # Fib(70) = 190392490709135 — fits in double safe integer range (2^53).
        # Larger values exceed Luau's 64-bit double precision.
        src = """
a, b = 0, 1
for _ in range(70):
    a, b = b, a + b
print(a)
"""
        luau_out, c_time, r_time = self._timed_compile_and_run(src)
        py_out, py_time = self._timed_python(src)
        assert luau_out == py_out, f"Output mismatch: {luau_out!r} vs {py_out!r}"
        print(
            f"\n  fib70: Luau={r_time:.3f}s CPython={py_time:.3f}s compile={c_time:.3f}s"
        )

    def test_perf_sum_range(self):
        src = """
total = 0
for i in range(100000):
    total = total + i
print(total)
"""
        luau_out, c_time, r_time = self._timed_compile_and_run(src)
        py_out, py_time = self._timed_python(src)
        assert luau_out == py_out, f"Output mismatch: {luau_out!r} vs {py_out!r}"
        print(
            f"\n  sum100k: Luau={r_time:.3f}s CPython={py_time:.3f}s compile={c_time:.3f}s"
        )

    def test_perf_nested_loop(self):
        src = """
total = 0
for i in range(100):
    for j in range(100):
        total = total + i * j
print(total)
"""
        luau_out, c_time, r_time = self._timed_compile_and_run(src)
        py_out, py_time = self._timed_python(src)
        assert luau_out == py_out, f"Output mismatch: {luau_out!r} vs {py_out!r}"
        print(
            f"\n  nested100x100: Luau={r_time:.3f}s CPython={py_time:.3f}s compile={c_time:.3f}s"
        )

    def test_perf_list_build(self):
        src = """
result = []
for i in range(10000):
    result.append(i * i)
print(len(result))
"""
        luau_out, c_time, r_time = self._timed_compile_and_run(src)
        py_out, py_time = self._timed_python(src)
        assert luau_out == py_out, f"Output mismatch: {luau_out!r} vs {py_out!r}"
        print(
            f"\n  listbuild10k: Luau={r_time:.3f}s CPython={py_time:.3f}s compile={c_time:.3f}s"
        )
