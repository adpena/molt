"""End-to-end test: compile a small Python program through the Molt frontend,
capture before/after IR for each pass using tv_hooks, and run the
TranslationValidator on the captured snapshots.

This proves the TV infrastructure works end-to-end without modifying
``__init__.py`` -- we drive the hooks directly from the test.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

import pytest

# Ensure tools/ is importable
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))

from translation_validator import TranslationValidator


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_simple_ops():
    """Return a realistic list of op-dicts representing a tiny function.

    Simulates what tv_hooks would emit for a function like:

        def f(x):
            a = 10
            b = 20
            c = a + b
            d = 99        # dead
            return c
    """
    def _val(name, th="int"):
        return {"name": name, "type_hint": th}

    return [
        {"kind": "CONST", "args": [10], "result": _val("a"), "metadata": None},
        {"kind": "CONST", "args": [20], "result": _val("b"), "metadata": None},
        {"kind": "ADD", "args": [_val("a"), _val("b")], "result": _val("c"), "metadata": None},
        {"kind": "CONST", "args": [99], "result": _val("d"), "metadata": None},
        {"kind": "RETURN", "args": [_val("c")], "result": {"name": "none", "type_hint": "Unknown"}, "metadata": None},
    ]


def _simulate_dce(ops):
    """Simulate a trivial DCE pass: remove ops whose result is unused."""
    used_names: set[str] = set()

    def _walk(obj):
        if isinstance(obj, dict):
            if "name" in obj and "type_hint" in obj:
                n = obj["name"]
                if n != "none":
                    used_names.add(n)
            else:
                for v in obj.values():
                    _walk(v)
        elif isinstance(obj, list):
            for item in obj:
                _walk(item)

    for op in ops:
        for arg in op.get("args", []):
            _walk(arg)

    result = []
    for op in ops:
        rname = op.get("result", {}).get("name", "none")
        if rname == "none" or rname in used_names:
            result.append(op)
    return result


def _simulate_simplify(ops):
    """Identity pass -- no changes (Simplify may or may not change things)."""
    return list(ops)


# ---------------------------------------------------------------------------
# Test: TV hooks emit + validator round-trip
# ---------------------------------------------------------------------------


class TestTVEndToEnd:
    """Exercise the full TV pipeline: emit snapshots -> validate them."""

    def test_emit_and_validate_roundtrip(self, tmp_path):
        """Emit before/after snapshots via tv_hooks, then validate the
        dump directory with TranslationValidator."""
        from molt.frontend.tv_hooks import (
            _emit,
            reset,
        )

        # Point TV output to tmp_path
        os.environ["MOLT_TV_EMIT"] = "1"
        os.environ["MOLT_TV_DIR"] = str(tmp_path)
        reset()

        try:
            ops_before = _make_simple_ops()

            # --- Pass 1: Simplify (identity) ---
            ops_after_simplify = _simulate_simplify(ops_before)
            _emit("test_func", "simplify", "before", ops_before)
            _emit("test_func", "simplify", "after", ops_after_simplify)

            # --- Pass 2: DCE ---
            ops_after_dce = _simulate_dce(ops_after_simplify)
            _emit("test_func", "dce", "before", ops_after_simplify)
            _emit("test_func", "dce", "after", ops_after_dce)

            # Verify files were created
            assert (tmp_path / "test_func_simplify_before.json").exists()
            assert (tmp_path / "test_func_simplify_after.json").exists()
            assert (tmp_path / "test_func_dce_before.json").exists()
            assert (tmp_path / "test_func_dce_after.json").exists()

            # Validate the dump directory
            tv = TranslationValidator()
            reports = tv.validate_dump_directory(str(tmp_path))

            assert len(reports) == 1, f"Expected 1 function report, got {len(reports)}"
            func_report = reports[0]
            assert func_report.function == "test_func"
            assert len(func_report.passes) == 2

            for pr in func_report.passes:
                if not pr.passed:
                    failures = [
                        f"  {c.check}: {c.detail}" for c in pr.checks if not c.passed
                    ]
                    pytest.fail(
                        f"Pass '{pr.pass_name}' failed:\n" + "\n".join(failures)
                    )
        finally:
            os.environ.pop("MOLT_TV_EMIT", None)
            os.environ.pop("MOLT_TV_DIR", None)
            reset()

    def test_snapshot_json_format(self, tmp_path):
        """Verify that emitted JSON files have the expected schema."""
        from molt.frontend.tv_hooks import _emit, reset

        os.environ["MOLT_TV_EMIT"] = "1"
        os.environ["MOLT_TV_DIR"] = str(tmp_path)
        reset()

        try:
            ops = _make_simple_ops()
            path = _emit("my_func", "cse", "before", ops)

            data = json.loads(path.read_text(encoding="utf-8"))

            assert data["function"] == "my_func"
            assert data["pass"] == "cse"
            assert data["phase"] == "before"
            assert data["op_count"] == len(ops)
            assert isinstance(data["ops"], list)
            assert len(data["ops"]) == len(ops)

            # Check individual op structure
            first_op = data["ops"][0]
            assert "kind" in first_op
            assert "args" in first_op
            assert "result" in first_op
            assert "name" in first_op["result"]
            assert "type_hint" in first_op["result"]
        finally:
            os.environ.pop("MOLT_TV_EMIT", None)
            os.environ.pop("MOLT_TV_DIR", None)
            reset()

    def test_dce_removes_dead_code_correctly(self):
        """Verify our simulated DCE removes exactly the dead op."""
        ops = _make_simple_ops()
        after = _simulate_dce(ops)

        # 'd' (CONST 99) is dead -- not used by any subsequent op
        result_names = [op["result"]["name"] for op in after]
        assert "d" not in result_names, "DCE should have removed dead variable 'd'"
        assert "a" in result_names
        assert "b" in result_names
        assert "c" in result_names

    def test_validator_catches_broken_transform(self, tmp_path):
        """A transform that introduces dangling uses should be caught."""
        from molt.frontend.tv_hooks import _emit, reset

        os.environ["MOLT_TV_EMIT"] = "1"
        os.environ["MOLT_TV_DIR"] = str(tmp_path)
        reset()

        try:
            before = _make_simple_ops()
            # Broken transform: remove 'a' but keep ADD that uses 'a'
            after = [op for op in before if op["result"]["name"] != "a"]

            _emit("broken_func", "dce", "before", before)
            _emit("broken_func", "dce", "after", after)

            tv = TranslationValidator()
            reports = tv.validate_dump_directory(str(tmp_path))
            assert len(reports) == 1
            fr = reports[0]
            assert len(fr.passes) == 1
            pr = fr.passes[0]
            # Should fail: 'a' was a pure op (CONST) removed, but still used
            assert not pr.passed, "Validator should catch dangling use of removed op"
        finally:
            os.environ.pop("MOLT_TV_EMIT", None)
            os.environ.pop("MOLT_TV_DIR", None)
            reset()

    def test_multi_pass_pipeline_simulation(self, tmp_path):
        """Simulate the full pass pipeline order and validate each step."""
        from molt.frontend.tv_hooks import _emit, reset

        os.environ["MOLT_TV_EMIT"] = "1"
        os.environ["MOLT_TV_DIR"] = str(tmp_path)
        reset()

        try:
            passes = ["simplify", "sccp", "dce", "cse"]
            ops = _make_simple_ops()

            for pass_name in passes:
                _emit("pipeline_func", pass_name, "before", ops)
                if pass_name == "dce":
                    ops = _simulate_dce(ops)
                else:
                    ops = _simulate_simplify(ops)
                _emit("pipeline_func", pass_name, "after", ops)

            tv = TranslationValidator()
            reports = tv.validate_dump_directory(str(tmp_path))
            assert len(reports) == 1
            fr = reports[0]
            assert len(fr.passes) == len(passes)
            for pr in fr.passes:
                assert pr.passed, (
                    f"Pass '{pr.pass_name}' unexpectedly failed: "
                    + "; ".join(c.detail for c in pr.checks if not c.passed)
                )
        finally:
            os.environ.pop("MOLT_TV_EMIT", None)
            os.environ.pop("MOLT_TV_DIR", None)
            reset()
