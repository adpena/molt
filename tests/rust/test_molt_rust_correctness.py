"""Rust backend correctness tests — differential testing via rustc vs CPython.

Each test compiles Python source to Rust via molt CLI, compiles with rustc,
runs the binary, and asserts identical stdout to CPython. This catches:
- Print formatting (int, float, bool, None, str)
- Arithmetic operator semantics (floor div, modulo, power, wrapping)
- Iteration lowering (range, for-in, nested, enumerate, zip)
- Data structure operations (list, dict, append, subscript)
- Control flow (if/else, while, break/continue, early return)
- Function semantics (recursion, closures, default args)
"""

import os
import shutil
import subprocess
import sys
import tempfile

import pytest

MOLT_DIR = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _find_rustc() -> str:
    """Return rustc path, preferring the active toolchain."""
    for candidate in ("rustc", os.path.expanduser("~/.cargo/bin/rustc")):
        try:
            r = subprocess.run(
                [candidate, "--version"], capture_output=True, text=True, timeout=15
            )
            if r.returncode == 0:
                return candidate
        except (FileNotFoundError, subprocess.TimeoutExpired):
            pass
    pytest.skip("rustc not found — install Rust to run Rust backend tests")


def _find_cpython() -> str:
    """Return a working CPython executable for baseline runs."""
    candidates: list[str] = []
    override = os.environ.get("MOLT_DIFF_PYTHON", "").strip()
    if override:
        candidates.append(override)
    candidates.extend(
        [
            sys.executable,
            getattr(sys, "_base_executable", ""),
            shutil.which("python3") or "",
            shutil.which("python") or "",
        ]
    )
    for candidate in candidates:
        if not candidate:
            continue
        if (os.sep in candidate or os.path.isabs(candidate)) and not os.path.exists(
            candidate
        ):
            continue
        try:
            probe = subprocess.run(
                [candidate, "-c", "import sys; print(sys.version_info[0])"],
                capture_output=True,
                text=True,
                timeout=5,
            )
            if probe.returncode == 0 and probe.stdout.strip() == "3":
                return candidate
        except (FileNotFoundError, subprocess.TimeoutExpired):
            continue
    pytest.skip("CPython executable not found for baseline comparison")


def _compile_and_run_rust(python_source: str, *, expect_fail: bool = False) -> str:
    """Compile Python → Rust via molt CLI, compile with rustc, run binary."""
    with tempfile.TemporaryDirectory() as tmpdir:
        py_path = os.path.join(tmpdir, "input.py")
        rs_path = os.path.join(tmpdir, "output.rs")
        bin_path = os.path.join(tmpdir, "output")

        with open(py_path, "w") as f:
            f.write(python_source)

        ext_root = os.environ.get("MOLT_EXT_ROOT", MOLT_DIR)
        cargo_target = os.environ.get(
            "CARGO_TARGET_DIR",
            os.path.join(ext_root, "target"),
        )
        env = {
            **os.environ,
            "MOLT_EXT_ROOT": ext_root,
            "CARGO_TARGET_DIR": cargo_target,
            "MOLT_USE_SCCACHE": "0",
            "MOLT_BACKEND_DAEMON": "0",
            "MOLT_DEV_CARGO_PROFILE": os.environ.get(
                "MOLT_DEV_CARGO_PROFILE", "release-fast"
            ),
            "MOLT_BUILD_STATE_DIR": os.environ.get(
                "MOLT_BUILD_STATE_DIR",
                os.path.join(ext_root, "tmp", f"rust-tests-build-state-{os.getpid()}"),
            ),
            "RUSTC_WRAPPER": "",
            "PYTHONPATH": os.path.join(MOLT_DIR, "src"),
            "UV_LINK_MODE": os.environ.get("UV_LINK_MODE", "copy"),
            "UV_NO_SYNC": os.environ.get("UV_NO_SYNC", "1"),
        }
        build_timeout = int(os.environ.get("MOLT_RUST_BUILD_TIMEOUT", "1200"))
        py_exec = sys.executable or _find_cpython()

        # Step 1: molt build --target rust
        try:
            result = subprocess.run(
                [
                    py_exec,
                    "-m",
                    "molt.cli",
                    "build",
                    py_path,
                    "--target",
                    "rust",
                    "--output",
                    rs_path,
                ],
                capture_output=True,
                text=True,
                timeout=build_timeout,
                env=env,
                cwd=MOLT_DIR,
            )
        except subprocess.TimeoutExpired as exc:
            pytest.fail(
                f"molt build --target rust timed out after {build_timeout}s ({exc.cmd})"
            )
        if result.returncode != 0:
            if expect_fail:
                return ""
            pytest.fail(
                f"molt build --target rust failed:\nstdout: {result.stdout}\nstderr: {result.stderr}"
            )

        # Step 2: rustc output.rs -o output
        rustc = _find_rustc()
        allow_lints = [
            "unused_mut",
            "unused_variables",
            "dead_code",
            "non_snake_case",
        ]
        result2 = subprocess.run(
            [
                rustc,
                rs_path,
                "-o",
                bin_path,
                "--edition=2021",
                *[flag for lint in allow_lints for flag in ("-A", lint)],
            ],
            capture_output=True,
            text=True,
            timeout=300,
        )
        if result2.returncode != 0:
            if expect_fail:
                return ""
            pytest.fail(f"rustc compilation failed:\n{result2.stderr}")

        # Step 3: run binary
        result3 = subprocess.run([bin_path], capture_output=True, text=True, timeout=30)
        return result3.stdout.strip()


def _cpython_stdout(python_source: str) -> str:
    """Run Python source with CPython and return stdout."""
    cpython = _find_cpython()
    r = subprocess.run(
        [cpython, "-c", python_source],
        capture_output=True,
        text=True,
        timeout=30,
    )
    return r.stdout.strip()


def assert_rust_eq_python(source: str) -> None:
    """Assert that Rust-compiled output matches CPython output."""
    expected = _cpython_stdout(source)
    actual = _compile_and_run_rust(source)
    assert actual == expected, (
        f"Rust output != CPython output\n"
        f"Source:\n{source}\n"
        f"Expected:\n{expected}\n"
        f"Got:\n{actual}"
    )


# ─── Basic print / literals ───────────────────────────────────────────────────


def test_print_int():
    assert_rust_eq_python("print(42)")


def test_print_negative():
    assert_rust_eq_python("print(-7)")


def test_print_float():
    assert_rust_eq_python("print(3.14)")


def test_print_string():
    assert_rust_eq_python('print("hello world")')


def test_print_bool_true():
    assert_rust_eq_python("print(True)")


def test_print_bool_false():
    assert_rust_eq_python("print(False)")


def test_print_none():
    assert_rust_eq_python("print(None)")


def test_print_multiple_args():
    assert_rust_eq_python("print(1, 2, 3)")


def test_print_mixed_args():
    assert_rust_eq_python('print("x =", 42)')


# ─── Arithmetic ───────────────────────────────────────────────────────────────


def test_add_ints():
    assert_rust_eq_python("print(3 + 4)")


def test_multiply():
    assert_rust_eq_python("print(6 * 7)")


def test_subtract():
    assert_rust_eq_python("print(10 - 3)")


def test_floor_division():
    assert_rust_eq_python("print(7 // 2)")


def test_modulo():
    assert_rust_eq_python("print(17 % 5)")


def test_power():
    assert_rust_eq_python("print(2 ** 10)")


def test_unary_neg():
    assert_rust_eq_python("x = 5; print(-x)")


def test_float_division():
    assert_rust_eq_python("print(10 / 4)")


def test_mixed_int_float():
    assert_rust_eq_python("print(1 + 2.5)")


# ─── Comparison / bool ────────────────────────────────────────────────────────


def test_eq():
    assert_rust_eq_python("print(1 == 1)")


def test_ne():
    assert_rust_eq_python("print(1 != 2)")


def test_list_not_equal_none():
    assert_rust_eq_python("print([1, 2] == None)")


def test_lt():
    assert_rust_eq_python("print(1 < 2)")


def test_gt():
    assert_rust_eq_python("print(3 > 2)")


def test_not():
    assert_rust_eq_python("print(not True)")


# ─── Variables ────────────────────────────────────────────────────────────────


def test_variable_assignment():
    assert_rust_eq_python("x = 10; y = 20; print(x + y)")


def test_augmented_assignment():
    assert_rust_eq_python("x = 5; x += 3; print(x)")


def test_swap():
    assert_rust_eq_python("a = 1; b = 2; a, b = b, a; print(a, b)")


# ─── Control flow ─────────────────────────────────────────────────────────────


def test_if_true():
    assert_rust_eq_python("if True:\n    print('yes')")


def test_if_false():
    assert_rust_eq_python("if False:\n    print('yes')\nelse:\n    print('no')")


def test_if_elif():
    assert_rust_eq_python(
        "x = 2\nif x == 1:\n    print('one')\nelif x == 2:\n    print('two')\nelse:\n    print('other')"
    )


def test_while_loop():
    assert_rust_eq_python("i = 0\nwhile i < 5:\n    i += 1\nprint(i)")


def test_while_break():
    assert_rust_eq_python(
        "i = 0\nwhile True:\n    if i == 3:\n        break\n    i += 1\nprint(i)"
    )


def test_while_continue():
    assert_rust_eq_python(
        "s = 0\ni = 0\nwhile i < 5:\n    i += 1\n    if i % 2 == 0:\n        continue\n    s += i\nprint(s)"
    )


# ─── For loops / range ────────────────────────────────────────────────────────


def test_for_range():
    assert_rust_eq_python("for i in range(5):\n    print(i)")


def test_for_range_start_stop():
    assert_rust_eq_python("for i in range(2, 7):\n    print(i)")


def test_for_range_step():
    assert_rust_eq_python("for i in range(0, 10, 2):\n    print(i)")


def test_for_range_negative_step():
    assert_rust_eq_python("for i in range(5, 0, -1):\n    print(i)")


def test_for_sum():
    assert_rust_eq_python("s = 0\nfor i in range(10):\n    s += i\nprint(s)")


# ─── Lists ────────────────────────────────────────────────────────────────────


def test_list_literal():
    assert_rust_eq_python("x = [1, 2, 3]\nprint(x[0])")


def test_list_append():
    assert_rust_eq_python("x = []\nx.append(1)\nx.append(2)\nprint(len(x))")


def test_nested_list_append_alias_writeback():
    assert_rust_eq_python(
        "rows = []\nrow = []\nrow.append(7)\nrows.append(row)\nprint(rows[0][0])"
    )


def test_function_local_nested_list_append_writeback():
    assert_rust_eq_python(
        "def build_rows():\n"
        "    rows = []\n"
        "    row = []\n"
        "    row.append(7)\n"
        "    rows.append(row)\n"
        "    return rows\n"
        "rows = build_rows()\n"
        "print(rows[0][0])"
    )


def test_list_index():
    assert_rust_eq_python("x = [10, 20, 30]\nprint(x[1])")


def test_list_negative_index():
    assert_rust_eq_python("x = [10, 20, 30]\nprint(x[-1])")


def test_list_len():
    assert_rust_eq_python("print(len([1, 2, 3, 4]))")


def test_for_list():
    assert_rust_eq_python("for x in [10, 20, 30]:\n    print(x)")


def test_list_sum():
    assert_rust_eq_python("print(sum([1, 2, 3, 4, 5]))")


# ─── Dict ─────────────────────────────────────────────────────────────────────


def test_dict_literal():
    assert_rust_eq_python("d = {'a': 1}\nprint(d['a'])")


def test_dict_set():
    assert_rust_eq_python("d = {}\nd['x'] = 42\nprint(d['x'])")


def test_dict_len():
    assert_rust_eq_python("d = {'a': 1, 'b': 2}\nprint(len(d))")


# ─── Functions ────────────────────────────────────────────────────────────────


def test_simple_function():
    assert_rust_eq_python("def add(a, b):\n    return a + b\nprint(add(3, 4))")


def test_recursive_function():
    assert_rust_eq_python(
        "def fact(n):\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\nprint(fact(10))"
    )


def test_fibonacci():
    assert_rust_eq_python(
        "def fib(n):\n    if n < 2:\n        return n\n    return fib(n-1) + fib(n-2)\nprint(fib(12))"
    )


def test_function_with_default_return():
    assert_rust_eq_python("def greet(name):\n    print('hello', name)\ngreet('world')")


def test_nested_function_calls():
    assert_rust_eq_python(
        "def double(x):\n    return x * 2\ndef quad(x):\n    return double(double(x))\nprint(quad(5))"
    )


# ─── String operations ────────────────────────────────────────────────────────


def test_string_len():
    assert_rust_eq_python("print(len('hello'))")


def test_string_concat():
    assert_rust_eq_python("print('hello' + ' ' + 'world')")


def test_str_conversion():
    assert_rust_eq_python("print(str(42))")


def test_int_conversion():
    assert_rust_eq_python("print(int('42') + 1)")


# ─── enumerate / zip ─────────────────────────────────────────────────────────


def test_enumerate():
    assert_rust_eq_python("for i, x in enumerate([10, 20, 30]):\n    print(i, x)")


def test_zip():
    assert_rust_eq_python("for a, b in zip([1,2,3], [4,5,6]):\n    print(a + b)")


# ─── min / max / abs ─────────────────────────────────────────────────────────


def test_min():
    assert_rust_eq_python("print(min(3, 7))")


def test_max():
    assert_rust_eq_python("print(max(3, 7))")


def test_abs():
    assert_rust_eq_python("print(abs(-5))")


# ─── End-to-end algorithm ─────────────────────────────────────────────────────


def test_bubble_sort():
    assert_rust_eq_python(
        "def sort(lst):\n"
        "    n = len(lst)\n"
        "    for i in range(n):\n"
        "        for j in range(0, n - i - 1):\n"
        "            if lst[j] > lst[j+1]:\n"
        "                lst[j], lst[j+1] = lst[j+1], lst[j]\n"
        "arr = [64, 34, 25, 12, 22, 11, 90]\n"
        "sort(arr)\n"
        "for x in arr:\n"
        "    print(x)"
    )


def test_matrix_multiply():
    assert_rust_eq_python(
        "def matmul(A, B):\n"
        "    n = len(A)\n"
        "    C = []\n"
        "    for i in range(n):\n"
        "        row = []\n"
        "        for j in range(n):\n"
        "            s = 0\n"
        "            for k in range(n):\n"
        "                s += A[i][k] * B[k][j]\n"
        "            row.append(s)\n"
        "        C.append(row)\n"
        "    return C\n"
        "A = [[1,2],[3,4]]\n"
        "B = [[5,6],[7,8]]\n"
        "C = matmul(A, B)\n"
        "for row in C:\n"
        "    print(row[0], row[1])"
    )


def test_collatz():
    assert_rust_eq_python(
        "def collatz(n):\n"
        "    steps = 0\n"
        "    while n != 1:\n"
        "        if n % 2 == 0:\n"
        "            n = n // 2\n"
        "        else:\n"
        "            n = n * 3 + 1\n"
        "        steps += 1\n"
        "    return steps\n"
        "print(collatz(27))"
    )
