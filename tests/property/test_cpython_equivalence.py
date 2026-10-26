"""Property-based CPython equivalence tests.

The META property: for randomly generated simple expressions, Molt and
CPython must produce identical output.  Uses custom Hypothesis strategies
to generate arithmetic and string expressions, then compares subprocess
outputs.

These tests require ``--run-molt`` and a working Molt compiler.
"""

from __future__ import annotations

import pytest
from hypothesis import given, settings, HealthCheck
from hypothesis import strategies as st
from hypothesis.strategies import composite

# ---------------------------------------------------------------------------
# Custom strategies for expression generation
# ---------------------------------------------------------------------------


@composite
def arithmetic_expressions(draw: st.DrawFn, max_depth: int = 3) -> str:
    """Generate a simple arithmetic expression string.

    Produces expressions like ``(3 + 7)``, ``((2 * -5) - 1)``, etc.
    All operands are small integers to keep results manageable.
    """
    if max_depth <= 0 or draw(st.booleans()):
        # Leaf: a small integer literal
        n = draw(st.integers(min_value=-100, max_value=100))
        # Parenthesize negatives to avoid ambiguity
        return str(n) if n >= 0 else f"({n})"

    op = draw(st.sampled_from(["+", "-", "*"]))
    left = draw(arithmetic_expressions(max_depth=max_depth - 1))
    right = draw(arithmetic_expressions(max_depth=max_depth - 1))

    return f"({left} {op} {right})"


@composite
def safe_divmod_expressions(draw: st.DrawFn) -> str:
    """Generate a division or modulo expression with a guaranteed non-zero divisor."""
    left = draw(st.integers(min_value=-100, max_value=100))
    right = draw(st.integers(min_value=1, max_value=50))
    op = draw(st.sampled_from(["//", "%"]))
    left_s = str(left) if left >= 0 else f"({left})"
    return f"({left_s} {op} {right})"


@composite
def string_expressions(draw: st.DrawFn) -> str:
    """Generate a simple string expression that produces a string result.

    Produces expressions like ``'hello' + 'world'``, ``'ab' * 3``,
    ``'HELLO'.lower()``, etc.
    """
    base = draw(
        st.text(
            alphabet=st.characters(min_codepoint=32, max_codepoint=126),
            min_size=0,
            max_size=15,
        )
    )
    # Escape for embedding in Python source
    base_repr = repr(base)

    op = draw(
        st.sampled_from(
            [
                "identity",
                "upper",
                "lower",
                "strip",
                "title",
                "concat",
                "repeat",
                "replace",
            ]
        )
    )

    if op == "identity":
        return base_repr
    elif op == "upper":
        return f"{base_repr}.upper()"
    elif op == "lower":
        return f"{base_repr}.lower()"
    elif op == "strip":
        return f"{base_repr}.strip()"
    elif op == "title":
        return f"{base_repr}.title()"
    elif op == "concat":
        other = draw(
            st.text(
                alphabet=st.characters(min_codepoint=32, max_codepoint=126),
                min_size=0,
                max_size=10,
            )
        )
        return f"{base_repr} + {repr(other)}"
    elif op == "repeat":
        n = draw(st.integers(min_value=0, max_value=5))
        return f"{base_repr} * {n}"
    elif op == "replace":
        old_char = draw(st.characters(min_codepoint=32, max_codepoint=126))
        new_char = draw(st.characters(min_codepoint=32, max_codepoint=126))
        return f"{base_repr}.replace({repr(old_char)}, {repr(new_char)})"
    else:
        return base_repr


# ---------------------------------------------------------------------------
# CPython-only equivalence tests (no Molt compilation needed)
# ---------------------------------------------------------------------------


class TestArithmeticExpressionGeneration:
    """Verify that generated arithmetic expressions are valid Python."""

    @given(expr=arithmetic_expressions())
    @settings(
        max_examples=200,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_arithmetic_expr_valid(self, expr: str) -> None:
        """Generated arithmetic expression produces output in CPython."""
        from tests.property.conftest import run_via_cpython

        code = f"print({expr})"
        result = run_via_cpython(code)
        # Should produce some output (a number)
        assert result.returncode == 0
        assert result.stderr == ""
        assert result.stdout not in (None, "")

    @given(expr=safe_divmod_expressions())
    @settings(
        max_examples=200,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_divmod_expr_valid(self, expr: str) -> None:
        """Generated divmod expression produces output in CPython."""
        from tests.property.conftest import run_via_cpython

        code = f"print({expr})"
        result = run_via_cpython(code)
        assert result.returncode == 0
        assert result.stderr == ""
        assert result.stdout not in (None, "")

    @given(expr=string_expressions())
    @settings(
        max_examples=200,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_string_expr_valid(self, expr: str) -> None:
        """Generated string expression produces output in CPython."""
        from tests.property.conftest import run_via_cpython

        code = f"print(repr({expr}))"
        result = run_via_cpython(code)
        assert result.returncode == 0
        assert result.stderr == ""
        assert result.stdout not in (None, "")


# ---------------------------------------------------------------------------
# Full Molt vs CPython equivalence (requires --run-molt)
# ---------------------------------------------------------------------------


@pytest.mark.molt_compile
class TestMoltCPythonArithmeticEquivalence:
    """For random arithmetic expressions, Molt output must match CPython."""

    @given(expr=arithmetic_expressions())
    @settings(
        max_examples=50,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_arithmetic_equivalence(self, expr: str) -> None:
        """Molt and CPython produce identical output for arithmetic expressions."""
        from tests.property.conftest import assert_molt_matches_cpython

        code = f"print({expr})"
        assert_molt_matches_cpython(code)

    @given(expr=safe_divmod_expressions())
    @settings(
        max_examples=50,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_divmod_equivalence(self, expr: str) -> None:
        """Molt and CPython produce identical output for divmod expressions."""
        from tests.property.conftest import assert_molt_matches_cpython

        code = f"print({expr})"
        assert_molt_matches_cpython(code)


@pytest.mark.molt_compile
class TestMoltCPythonStringEquivalence:
    """For random string expressions, Molt output must match CPython."""

    @given(expr=string_expressions())
    @settings(
        max_examples=50,
        deadline=None,
        database=None,
        suppress_health_check=[HealthCheck.too_slow],
    )
    def test_string_equivalence(self, expr: str) -> None:
        """Molt and CPython produce identical output for string expressions."""
        from tests.property.conftest import assert_molt_matches_cpython

        code = f"print(repr({expr}))"
        assert_molt_matches_cpython(code)


@pytest.mark.molt_compile
class TestMoltCPythonBuiltinEquivalence:
    """For simple builtin calls, Molt output must match CPython."""

    @given(n=st.integers(min_value=-1000, max_value=1000))
    @settings(max_examples=50, deadline=None, database=None)
    def test_abs_equivalence(self, n: int) -> None:
        """abs() matches between Molt and CPython."""
        from tests.property.conftest import assert_molt_matches_cpython

        assert_molt_matches_cpython(f"print(abs({n}))")

    @given(
        a=st.integers(min_value=-100, max_value=100),
        b=st.integers(min_value=-100, max_value=100),
    )
    @settings(max_examples=50, deadline=None, database=None)
    def test_min_max_equivalence(self, a: int, b: int) -> None:
        """min() and max() match between Molt and CPython."""
        from tests.property.conftest import assert_molt_matches_cpython

        assert_molt_matches_cpython(f"print(min({a}, {b}), max({a}, {b}))")

    @given(n=st.integers(min_value=0, max_value=50))
    @settings(max_examples=50, deadline=None, database=None)
    def test_range_equivalence(self, n: int) -> None:
        """list(range(n)) matches between Molt and CPython."""
        from tests.property.conftest import assert_molt_matches_cpython

        assert_molt_matches_cpython(f"print(list(range({n})))")

    @given(n=st.integers(min_value=-1000, max_value=1000))
    @settings(max_examples=50, deadline=None, database=None)
    def test_bool_equivalence(self, n: int) -> None:
        """bool() matches between Molt and CPython."""
        from tests.property.conftest import assert_molt_matches_cpython

        assert_molt_matches_cpython(f"print(bool({n}))")

    @given(n=st.integers(min_value=-(2**46), max_value=2**46 - 1))
    @settings(max_examples=50, deadline=None, database=None)
    def test_int_str_roundtrip_equivalence(self, n: int) -> None:
        """int(str(n)) roundtrip matches between Molt and CPython."""
        from tests.property.conftest import assert_molt_matches_cpython

        assert_molt_matches_cpython(f"print(int(str({n})))")


class TestPropertyProcessContract:
    """Independent controls for the property runner's observable boundary."""

    def test_cpython_preserves_bytes_and_guest_failure(self) -> None:
        from tests.property.conftest import run_via_cpython
        from tools.compat.comparison import Outputs

        result = run_via_cpython(
            "import os; os.write(1, b' \\r\\n\\n\\x00\\xff '); "
            "os.write(2, b'\\xfe\\n\\n'); raise SystemExit(7)"
        )
        assert result == Outputs(" \r\n\n\x00\udcff ", "\udcfe\n\n", 7)

    @pytest.mark.parametrize(
        "candidate",
        [
            "import os; os.write(1, b'value'); os.write(2, b'diagnostic\\n')",
            "import os; os.write(1, b'value\\n\\n'); os.write(2, b'diagnostic\\n')",
            "import os; os.write(1, b'value\\n')",
            "import os; os.write(1, b'value\\n'); os.write(2, b'diagnostic\\n'); raise SystemExit(7)",
        ],
        ids=["missing-newline", "extra-blank-line", "missing-stderr", "exit-status"],
    )
    def test_comparison_rejects_changed_observables(
        self, monkeypatch, candidate
    ) -> None:
        from tests.property import conftest as helper

        reference = "import os; os.write(1, b'value\\n'); os.write(2, b'diagnostic\\n')"
        monkeypatch.setattr(
            helper, "run_via_molt", lambda code: helper.run_via_cpython(candidate)
        )
        with pytest.raises(AssertionError, match="Molt/CPython mismatch"):
            helper.assert_molt_matches_cpython(reference)

    def test_comparison_accepts_matching_nonzero_guest_exit(self, monkeypatch) -> None:
        from tests.property import conftest as helper

        monkeypatch.setattr(helper, "run_via_molt", helper.run_via_cpython)
        helper.assert_molt_matches_cpython(
            "import os; os.write(2, b'expected failure\\n'); raise SystemExit(7)"
        )

    @pytest.mark.parametrize(
        "failure",
        [
            "cancelled",
            "descendants_closed",
            "timed_out",
            "infrastructure_failure",
            "violation",
        ],
    )
    def test_harness_failure_cannot_be_guest_output(self, monkeypatch, failure) -> None:
        from tests.property import conftest as helper
        from tools.harness_memory_guard import GuardedCompletedProcess
        from tools.memory_guard_core.process_custody import GuardInfrastructureFailure

        process = GuardedCompletedProcess(
            ["guest"],
            0,
            b"value\n",
            b"guard failure\n",
            elapsed_s=0.0,
            child_stderr=b"",
            descendants_closed=True,
        )
        value = (
            GuardInfrastructureFailure(
                phase="temporary_artifact_custody", details=("lost custody",)
            )
            if failure == "infrastructure_failure"
            else failure != "descendants_closed"
        )
        setattr(process, failure, value)
        monkeypatch.setattr(
            helper, "run_guarded_test_process", lambda *a, **kw: process
        )
        with pytest.raises(RuntimeError, match="did not complete") as raised:
            helper.run_via_cpython("pass")
        assert raised.value.guarded_result is process

    def test_guard_diagnostics_do_not_replace_guest_stderr(self, monkeypatch) -> None:
        from tests.property import conftest as helper
        from tools.harness_memory_guard import GuardedCompletedProcess
        from tools.compat.comparison import Outputs

        process = GuardedCompletedProcess(
            ["guest"],
            7,
            b"",
            b"expected\nreproduction command\n",
            elapsed_s=0.0,
            child_stderr=b"expected\n",
            child_returncode=7,
            descendants_closed=True,
        )
        monkeypatch.setattr(
            helper, "run_guarded_test_process", lambda *a, **kw: process
        )
        assert helper.run_via_cpython("pass") == Outputs("", "expected\n", 7)

    def test_molt_runs_only_selected_build_output(self, tmp_path, monkeypatch) -> None:
        import json
        from tests.property import conftest as helper
        from tools.harness_memory_guard import GuardedCompletedProcess
        from tools.compat.comparison import Outputs

        selected = tmp_path / "selected artifact.exe"
        selected.write_bytes(b"selected artifact")
        commands = []

        def run(command, **kwargs):
            commands.append(command)
            assert kwargs["text"] is False
            if len(commands) == 1:
                assert "--json" in command
                source = Path(command[-1])
                assert source.read_text(encoding="utf-8") == "print('value')"
                (source.parent / "prop_test").write_bytes(b"decoy")
                payload = {
                    "status": "ok",
                    "data": {
                        "output": str(source.parent / "not-the-consumer"),
                        "consumer_output": str(selected),
                    },
                }
                return GuardedCompletedProcess(
                    command,
                    0,
                    json.dumps(payload).encode(),
                    b"compiler warning\n",
                    elapsed_s=0.0,
                    child_stderr=b"compiler warning\n",
                    descendants_closed=True,
                )
            assert command == [str(selected)]
            return GuardedCompletedProcess(
                command,
                7,
                b"value\n",
                b"guest warning\n",
                elapsed_s=0.0,
                child_stderr=b"guest warning\n",
                descendants_closed=True,
            )

        from pathlib import Path

        monkeypatch.setattr(helper, "run_guarded_test_process", run)
        result = helper.run_via_molt("print('value')")
        assert result == Outputs("value\n", "guest warning\n", 7)
        assert len(commands) == 2
        assert not Path(commands[0][-1]).parent.exists()

    @pytest.mark.parametrize("closed", [False, True])
    @pytest.mark.parametrize("phase", ["build", "guest"])
    def test_scratch_lifetime_requires_process_closure(
        self, tmp_path, monkeypatch, closed, phase
    ) -> None:
        import json
        from pathlib import Path
        from tests.property import conftest as helper
        from molt.temporary_artifacts import OwnedTemporaryDirectory
        from tools.harness_memory_guard import GuardedCompletedProcess
        from tools.memory_guard_core.process_custody import GuardInfrastructureFailure

        owners = []
        calls = []

        def allocate(**kwargs):
            owner = OwnedTemporaryDirectory(dir=tmp_path, **kwargs)
            owners.append(owner)
            return owner

        def run(command, **kwargs):
            calls.append(command)
            if len(calls) == 1 and phase == "guest":
                binary = Path(owners[0].name) / "selected"
                binary.write_bytes(b"fixture")
                return GuardedCompletedProcess(
                    command,
                    0,
                    json.dumps(
                        {"status": "ok", "data": {"output": str(binary)}}
                    ).encode(),
                    b"",
                    elapsed_s=0.0,
                    child_stderr=b"",
                    descendants_closed=True,
                )
            return GuardedCompletedProcess(
                command,
                125,
                b"",
                b"lost custody",
                elapsed_s=0.0,
                child_stderr=b"",
                descendants_closed=closed,
                infrastructure_failure=GuardInfrastructureFailure(
                    phase="temporary_artifact_custody", details=("lost custody",)
                ),
            )

        monkeypatch.setattr(helper, "OwnedTemporaryDirectory", allocate)
        monkeypatch.setattr(helper, "run_guarded_test_process", run)
        try:
            with pytest.raises(RuntimeError, match="did not complete") as raised:
                helper.run_via_molt("print('value')")
            assert Path(owners[0].name).exists() is not closed
            if not closed:
                assert any(owners[0].name in note for note in raised.value.__notes__)
        finally:
            # This fixture launches no child; retire its deliberately retained owner.
            for owner in owners:
                owner.cleanup()

    @pytest.mark.parametrize(
        "fault",
        [
            "build-error",
            "malformed-json",
            "error-status",
            "missing-output",
            "directory",
        ],
    )
    def test_invalid_build_never_executes_a_guest(
        self, tmp_path, monkeypatch, fault
    ) -> None:
        import json
        import subprocess
        from pathlib import Path
        from tests.property import conftest as helper
        from tools.harness_memory_guard import GuardedCompletedProcess

        calls = []

        def run(command, **kwargs):
            calls.append(command)
            assert len(calls) == 1
            payload = {"status": "ok", "data": {"output": str(tmp_path)}}
            if fault == "missing-output":
                payload["data"] = {}
            elif fault == "error-status":
                payload["status"] = "error"
            output = (
                b"not JSON"
                if fault == "malformed-json"
                else json.dumps(payload).encode()
            )
            return GuardedCompletedProcess(
                command,
                1 if fault == "build-error" else 0,
                output,
                b"compiler diagnostic",
                elapsed_s=0.0,
                child_stderr=b"compiler diagnostic",
                descendants_closed=True,
            )

        monkeypatch.setattr(helper, "run_guarded_test_process", run)
        with pytest.raises(
            (subprocess.CalledProcessError, ValueError, RuntimeError, FileNotFoundError)
        ):
            helper.run_via_molt("print('value')")
        assert len(calls) == 1
        assert not Path(calls[0][-1]).parent.exists()
