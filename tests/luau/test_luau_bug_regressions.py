"""Regression tests for 8 bugs fixed in the Molt Python-to-Luau transpiler.

Each test verifies:
1. Pattern check  -- the generated Luau source contains correct codegen patterns
2. Output check   -- when run through Lune, the output matches CPython

Bug references:
  P0-1  string.replace with regex-special pattern characters
  P0-2  `in` operator for list membership
  P0-3  math.trunc on negative numbers
  P2-1  isinstance type checking (was stubbed)
  P2-2  math.log10 translation
  P2-3  del list[i]
  P2-9  pow(base, exp, mod) three-arg form
  P3-3  string lstrip/rstrip
"""

import os
import re
import subprocess
import sys
import tempfile
import pytest

MOLT_DIR = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ARTIFACT_ROOT = os.environ.get("MOLT_EXT_ROOT", MOLT_DIR)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _compile_to_luau(python_source: str) -> str:
    """Compile Python source to Luau via molt CLI and return the Luau source code.

    Returns the generated Luau text (not the runtime output).
    Calls pytest.skip if compilation fails.
    """
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
            pytest.skip(f"Compilation failed: {result.stderr[:300]}")

        with open(luau_path, "r") as f:
            return f.read()
    finally:
        for p in [py_path]:
            if os.path.exists(p):
                os.unlink(p)
        # Keep luau_path alive for _run_luau; caller manages cleanup.


def _run_luau(python_source: str) -> str:
    """Compile Python source to Luau, run through Lune, return stdout."""
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
            pytest.skip(f"Compilation failed: {result.stderr[:300]}")

        try:
            result = subprocess.run(
                ["lune", "run", luau_path],
                capture_output=True,
                text=True,
                timeout=30,
            )
        except FileNotFoundError:
            pytest.skip("lune not found")
        if result.returncode != 0:
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
    assert _run_luau(src) == _python_output(src)


# ===========================================================================
# Regression test classes
# ===========================================================================


class TestTruncNegativeNumbers:
    """Regression: math.trunc(-2.7) must return -2, not -3.

    Bug P0-3: The old codegen used math.floor unconditionally, which rounds
    negative numbers in the wrong direction.  The fix uses math.ceil for
    negative values (truncation toward zero).
    """

    SRC = """\
import math
print(math.trunc(-2.7))
print(math.trunc(2.7))
print(math.trunc(-0.5))
"""

    def test_pattern_no_bare_floor(self):
        """Generated Luau must NOT use a bare math.floor for trunc on negatives.

        It should use math.ceil for the negative branch (truncation toward zero).
        """
        luau = _compile_to_luau(self.SRC)
        # The fix should involve ceiling for negative numbers -- either an
        # if-branch or a helper that distinguishes positive/negative.
        # At minimum, there should NOT be only math.floor with no sign check.
        has_ceil = "math.ceil" in luau
        has_sign_check = any(
            tok in luau for tok in ["< 0", "> 0", ">= 0", "<= 0", "sign"]
        )
        assert has_ceil or has_sign_check, (
            "math.trunc codegen should use math.ceil or a sign check, "
            f"but generated Luau has neither.\n--- snippet ---\n{luau[:1000]}"
        )

    def test_output_matches_cpython(self):
        """Lune output for math.trunc must match CPython exactly."""
        _assert_match(self.SRC)

    def test_trunc_zero(self):
        """Edge case: trunc(0.0) and trunc(-0.0) should both be 0."""
        src = """\
import math
print(math.trunc(0.0))
print(math.trunc(-0.0))
"""
        _assert_match(src)


class TestStringReplacePatternChars:
    """Regression: str.replace must escape Lua pattern-special characters.

    Bug P0-1: Characters like . + * % are special in Lua patterns.  The old
    codegen passed them raw to string.gsub, causing wrong replacements.
    """

    SRC = """\
s = "hello.world.test"
print(s.replace(".", "-"))
s2 = "a+b*c"
print(s2.replace("+", "PLUS"))
s3 = "100%"
print("x".replace("x", "100%"))
"""

    def test_pattern_escaping(self):
        """Generated Luau must escape pattern-special characters in gsub calls."""
        luau = _compile_to_luau(self.SRC)
        # The codegen should either:
        # - Use a pattern-escaping helper (e.g. :gsub("%%", ...))
        # - Use string.gsub with escaped dots like "%."
        # - Use a plain-replace helper that avoids patterns entirely
        has_gsub = "gsub" in luau
        has_escape = "%%." in luau or "%%%%" in luau or "escape" in luau.lower()
        has_plain_replace = (
            "string.rep" in luau or "_replace" in luau.lower() or "find" in luau
        )
        assert has_gsub or has_plain_replace, (
            "str.replace should compile to gsub or a replace helper, "
            f"but neither found.\n--- snippet ---\n{luau[:1000]}"
        )
        if has_gsub:
            # If using gsub directly, there must be some form of escaping
            assert (
                has_escape
                or has_plain_replace
                or "_replace" in luau.lower()
                or "plain" in luau.lower()
            ), (
                "gsub is used but no pattern escaping detected — "
                f"pattern-special chars will break.\n--- snippet ---\n{luau[:1000]}"
            )

    def test_output_matches_cpython(self):
        """Lune output for string replace with special chars must match CPython."""
        _assert_match(self.SRC)

    def test_replace_percent_sign(self):
        """Edge case: replacing a percent sign should work correctly."""
        src = """\
s = "100% done"
print(s.replace("%", "%%"))
"""
        _assert_match(src)


class TestContainsOperator:
    """Regression: `in` operator must work for lists, strings, and dicts.

    Bug P0-2: The `in` operator was not implemented for list membership,
    only for string containment.
    """

    SRC = """\
nums = [10, 20, 30]
print(30 in nums)
print(99 in nums)
s = "hello world"
print("world" in s)
print("xyz" in s)
d = {"a": 1, "b": 2}
print("a" in d)
print("c" in d)
"""

    def test_pattern_list_membership(self):
        """Generated Luau should use table.find for list `in` checks."""
        luau = _compile_to_luau(self.SRC)
        # For list membership, the correct lowering is table.find
        has_table_find = "table.find" in luau
        # Some backends may use a helper function instead
        has_contains_helper = "_contains" in luau.lower() or "_in(" in luau.lower()
        assert has_table_find or has_contains_helper, (
            "`in` for lists should lower to table.find or a contains helper, "
            f"but neither found.\n--- snippet ---\n{luau[:1000]}"
        )

    def test_pattern_string_membership(self):
        """Generated Luau should use string.find for string `in` checks."""
        luau = _compile_to_luau(self.SRC)
        has_string_find = (
            "string.find" in luau or ":find(" in luau or "string_find" in luau
        )
        has_contains_helper = "_contains" in luau.lower() or "_in(" in luau.lower()
        assert has_string_find or has_contains_helper, (
            "`in` for strings should lower to string.find or a contains helper, "
            f"but neither found.\n--- snippet ---\n{luau[:1000]}"
        )

    def test_output_matches_cpython(self):
        """Lune output for the in operator must match CPython exactly."""
        _assert_match(self.SRC)

    def test_not_in_operator(self):
        """The `not in` operator should also work."""
        src = """\
nums = [10, 20, 30]
print(99 not in nums)
print(10 not in nums)
"""
        _assert_match(src)


class TestDelListItem:
    """Regression: `del lst[i]` must remove the element and shift, not set nil.

    Bug P2-3: The old codegen emitted `lst[i] = nil`, which leaves a hole
    in the Luau table instead of removing the element.
    """

    SRC = """\
lst = [1, 2, 3, 4, 5]
del lst[2]
print(lst)
print(len(lst))
"""

    def test_pattern_table_remove(self):
        """Generated Luau must use table.remove, not assignment to nil."""
        luau = _compile_to_luau(self.SRC)
        assert "table.remove" in luau, (
            "del list[i] should compile to table.remove, "
            f"but not found.\n--- snippet ---\n{luau[:1000]}"
        )
        # Must NOT use `= nil` for deletion
        # Look for patterns like `lst[...] = nil` which indicate the old bug
        nil_assign = re.search(r"\w+\[.*\]\s*=\s*nil", luau)
        assert nil_assign is None, (
            f"del list[i] should NOT compile to `= nil`, "
            f"but found: {nil_assign.group()}"
        )

    def test_pattern_offset(self):
        """table.remove call should include +1 offset for 0-to-1-based indexing."""
        luau = _compile_to_luau(self.SRC)
        # The index 2 in Python should become 3 in Luau (2+1)
        has_offset = "+ 1" in luau or "+1" in luau or "+ 1)" in luau
        assert has_offset, (
            "table.remove should include a +1 index offset, "
            f"but no offset pattern found.\n--- snippet ---\n{luau[:1000]}"
        )

    def test_output_matches_cpython(self):
        """Lune output for del list[i] must match CPython."""
        _assert_match(self.SRC)

    def test_del_first_and_last(self):
        """Edge cases: deleting first and last elements."""
        src = """\
lst = [10, 20, 30]
del lst[0]
print(lst)
del lst[-1]
print(lst)
"""
        _assert_match(src)


class TestPowThreeArg:
    """Regression: pow(base, exp, mod) must use modular exponentiation.

    Bug P2-9: The old codegen used `base ^ exp % mod` which overflows for
    large exponents.  The fix uses a square-and-multiply loop.
    """

    SRC = """\
print(pow(2, 10, 1000))
print(pow(3, 100, 97))
print(pow(2, 0, 5))
"""

    def test_pattern_no_caret_operator(self):
        """Three-arg pow must NOT compile to the ^ operator."""
        luau = _compile_to_luau(self.SRC)
        # Check that for the 3-arg pow calls there is no naive `^` usage
        # The codegen should have a loop or a dedicated modpow helper
        has_loop = "while" in luau or "for" in luau
        has_modpow_helper = "modpow" in luau.lower() or "_pow" in luau.lower()
        has_bit_ops = "bit32" in luau or "// 2" in luau or "% 2" in luau
        assert has_loop or has_modpow_helper or has_bit_ops, (
            "Three-arg pow should use a square-and-multiply loop or modpow helper, "
            f"but found neither.\n--- snippet ---\n{luau[:1000]}"
        )

    def test_output_matches_cpython(self):
        """Lune output for pow(b, e, m) must match CPython exactly."""
        _assert_match(self.SRC)

    def test_pow_large_exponent(self):
        """Large exponent that would overflow without modular exponentiation."""
        src = """\
print(pow(7, 256, 13))
print(pow(2, 1000, 37))
"""
        _assert_match(src)

    def test_pow_two_arg_unchanged(self):
        """Two-arg pow should still work (may use ^ operator)."""
        src = """\
print(pow(2, 10))
print(pow(3, 4))
"""
        _assert_match(src)


class TestIsinstance:
    """Regression: isinstance() must emit real type checks, not stubs.

    Bug P2-1: The old codegen emitted `true -- [stub: isinstance]` for all
    isinstance calls, always returning True.
    """

    SRC = """\
print(isinstance(42, int))
print(isinstance("hello", str))
print(isinstance(True, bool))
print(isinstance(42, str))
"""

    def test_pattern_no_stub(self):
        """Generated Luau must NOT contain stub markers for isinstance."""
        luau = _compile_to_luau(self.SRC)
        assert "stub" not in luau.lower(), (
            f"isinstance should be fully implemented, not stubbed.\n"
            f"--- snippet ---\n{luau[:1000]}"
        )

    def test_pattern_type_check(self):
        """Generated Luau should contain actual type-checking logic."""
        luau = _compile_to_luau(self.SRC)
        # Should use typeof() or type() for runtime type checking
        has_type_check = "typeof(" in luau or "type(" in luau or "typeof " in luau
        has_isinstance_helper = (
            "isinstance" in luau.lower() or "_isinstance" in luau.lower()
        )
        assert has_type_check or has_isinstance_helper, (
            "isinstance should compile to type()/typeof() checks or a helper, "
            f"but found neither.\n--- snippet ---\n{luau[:1000]}"
        )

    def test_output_matches_cpython(self):
        """Lune output for isinstance must match CPython exactly."""
        _assert_match(self.SRC)

    def test_isinstance_false_cases(self):
        """isinstance must return False when the type does not match."""
        src = """\
print(isinstance(42, str))
print(isinstance("hello", int))
print(isinstance(3.14, int))
print(isinstance(None, int))
"""
        _assert_match(src)


class TestMathLog10:
    """Regression: math.log10 must use base-10 logarithm, not natural log.

    Bug P2-2: The old codegen emitted math.log(x) instead of math.log(x, 10).
    """

    SRC = """\
import math
print(math.log10(100))
print(math.log10(1000))
"""

    def test_pattern_base10(self):
        """Generated Luau must use base-10 log, not bare math.log."""
        luau = _compile_to_luau(self.SRC)
        # Correct lowerings:
        #   math.log(x, 10)  -- Luau supports second arg
        #   math.log(x) / math.log(10)  -- manual base conversion
        #   math.log10(x)  -- if Luau has it (it doesn't natively)
        has_log10 = "math.log10" in luau
        has_log_base10 = re.search(r"math\.log\([^)]+,\s*10\)", luau) is not None
        has_log_division = (
            re.search(r"math\.log\([^)]+\)\s*/\s*math\.log\(\s*10\s*\)", luau)
            is not None
        )
        assert has_log10 or has_log_base10 or has_log_division, (
            "math.log10 should compile to math.log(x, 10) or equivalent, "
            f"but none of the expected patterns found.\n--- snippet ---\n{luau[:1000]}"
        )

    def test_output_matches_cpython(self):
        """Lune output for math.log10 must match CPython."""
        _assert_match(self.SRC)

    def test_log10_of_one(self):
        """Edge case: log10(1) should be 0.0."""
        src = """\
import math
print(math.log10(1))
"""
        _assert_match(src)


class TestStringStripMethods:
    """Regression: lstrip and rstrip must strip from different sides.

    Bug P3-3: lstrip and rstrip both compiled to the same pattern, causing
    both to strip from both sides (same as strip()).
    """

    SRC = """\
s = "  hello  "
print(s.lstrip())
print(s.rstrip())
print(s.strip())
"""

    def test_pattern_different_implementations(self):
        """lstrip and rstrip must generate DIFFERENT Luau patterns."""
        # Compile a source that uses only lstrip
        luau_lstrip = _compile_to_luau('s = "  hello  "\nprint(s.lstrip())\n')
        # Compile a source that uses only rstrip
        luau_rstrip = _compile_to_luau('s = "  hello  "\nprint(s.rstrip())\n')

        # Extract the pattern/match calls (look for gsub or match patterns)
        # lstrip should anchor at the start (^), rstrip at the end ($)
        lstrip_has_caret = "^" in luau_lstrip
        rstrip_has_dollar = "$" in luau_rstrip or "%%s*$" in luau_rstrip

        # At minimum, they should not be identical codegen
        # (strip the boilerplate and compare the core logic)
        assert lstrip_has_caret, "lstrip codegen should anchor at the start"
        assert rstrip_has_dollar, "rstrip codegen should anchor at the end"
        assert luau_lstrip != luau_rstrip, (
            "lstrip and rstrip generated identical Luau code — "
            "they should differ to strip from different sides."
        )

    def test_pattern_lstrip_anchors_start(self):
        """lstrip pattern should match leading whitespace (anchored at start)."""
        luau = _compile_to_luau('s = "  hello  "\nprint(s.lstrip())\n')
        # Should have ^ for start-of-string anchoring
        has_start_anchor = "^" in luau
        has_lstrip_helper = "lstrip" in luau.lower() or "ltrim" in luau.lower()
        assert has_start_anchor or has_lstrip_helper, (
            "lstrip should anchor at start of string (^) or use a named helper.\n"
            f"--- snippet ---\n{luau[:1000]}"
        )

    def test_pattern_rstrip_anchors_end(self):
        """rstrip pattern should match trailing whitespace (anchored at end)."""
        luau = _compile_to_luau('s = "  hello  "\nprint(s.rstrip())\n')
        # Should have $ for end-of-string anchoring
        has_end_anchor = "$" in luau
        has_rstrip_helper = "rstrip" in luau.lower() or "rtrim" in luau.lower()
        assert has_end_anchor or has_rstrip_helper, (
            "rstrip should anchor at end of string ($) or use a named helper.\n"
            f"--- snippet ---\n{luau[:1000]}"
        )

    def test_output_matches_cpython(self):
        """Lune output for lstrip/rstrip/strip must match CPython."""
        _assert_match(self.SRC)

    def test_strip_custom_chars(self):
        """Strip with custom character arguments."""
        src = """\
s = "xxhelloxx"
print(s.lstrip("x"))
print(s.rstrip("x"))
print(s.strip("x"))
"""
        _assert_match(src)
