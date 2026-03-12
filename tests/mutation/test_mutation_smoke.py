"""Smoke tests for the mutation testing framework (MOL-282).

These tests verify that the mutation infrastructure itself works
correctly — discovery, application, workspace isolation — without
running the full differential suite.
"""

from __future__ import annotations

import ast
import sys
import textwrap
from pathlib import Path

# Ensure tools/ is importable.
REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "tools"))

from mutation_test import (  # noqa: E402
    MutationSite,
    apply_single_mutation,
    create_mutant_workspace,
    discover_mutations,
)


# ---------------------------------------------------------------------------
# Discovery tests
# ---------------------------------------------------------------------------


SAMPLE_SOURCE = textwrap.dedent("""\
    def add(a, b):
        return a + b

    def check(x):
        if x == 0:
            return True
        return x > 1 and x < 100

    def count():
        total = 0
        for i in range(10):
            total += 1
        return total
""")


def test_discover_arith_op() -> None:
    """ArithSwap should find the + in add()."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"arith_op"})
    assert len(sites) >= 1
    arith = [s for s in sites if s.operator == "arith_op"]
    assert any("Add -> Sub" in s.description for s in arith)


def test_discover_cmp_op() -> None:
    """CompSwap should find == and > and < in check()."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"cmp_op"})
    cmp_sites = [s for s in sites if s.operator == "cmp_op"]
    assert len(cmp_sites) >= 2
    descs = {s.description for s in cmp_sites}
    assert any("Eq" in d for d in descs)
    assert any("Gt" in d or "Lt" in d for d in descs)


def test_discover_bool_op() -> None:
    """BoolOp should find the 'and' in check()."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"bool_op"})
    bool_sites = [s for s in sites if s.operator == "bool_op"]
    assert len(bool_sites) >= 1
    assert any("And -> Or" in s.description for s in bool_sites)


def test_discover_const_replace() -> None:
    """ConstReplace should find 0 and True and 1 constants."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"const_replace"})
    const_sites = [s for s in sites if s.operator == "const_replace"]
    assert len(const_sites) >= 2
    descs = {s.description for s in const_sites}
    assert any("0 -> 1" in d for d in descs)
    assert any("True -> False" in d for d in descs)


def test_discover_return_mutate() -> None:
    """ReturnMutate should find returns with values."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"return_mutate"})
    ret_sites = [s for s in sites if s.operator == "return_mutate"]
    assert len(ret_sites) >= 2


def test_discover_stmt_delete() -> None:
    """StmtDelete should find assignment statements in functions."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"stmt_delete"})
    stmt_sites = [s for s in sites if s.operator == "stmt_delete"]
    assert len(stmt_sites) >= 1


def test_discover_all_operators() -> None:
    """All operators together should find many sites."""
    all_ops = {
        "arith_op",
        "cmp_op",
        "bool_op",
        "const_replace",
        "stmt_delete",
        "return_mutate",
    }
    sites = discover_mutations("<test>", SAMPLE_SOURCE, all_ops)
    # We expect at least one of each operator type in our sample.
    found_ops = {s.operator for s in sites}
    assert found_ops == all_ops, f"Missing operators: {all_ops - found_ops}"


# ---------------------------------------------------------------------------
# Application tests
# ---------------------------------------------------------------------------


def test_apply_arith_mutation() -> None:
    """Applying an arith_op mutation should change + to -."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"arith_op"})
    add_site = next(s for s in sites if "Add -> Sub" in s.description)
    mutated = apply_single_mutation(SAMPLE_SOURCE, add_site)
    assert mutated is not None
    # The mutated source should contain subtraction where addition was.
    assert "a - b" in mutated or "a-b" in mutated


def test_apply_cmp_mutation() -> None:
    """Applying a cmp_op mutation should flip == to !=."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"cmp_op"})
    eq_site = next(s for s in sites if "Eq -> NotEq" in s.description)
    mutated = apply_single_mutation(SAMPLE_SOURCE, eq_site)
    assert mutated is not None
    assert "!=" in mutated


def test_apply_bool_mutation() -> None:
    """Applying a bool_op mutation should flip and to or."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"bool_op"})
    and_site = next(s for s in sites if "And -> Or" in s.description)
    mutated = apply_single_mutation(SAMPLE_SOURCE, and_site)
    assert mutated is not None
    assert " or " in mutated


def test_apply_return_mutation() -> None:
    """Applying return_mutate should replace return value with None."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"return_mutate"})
    ret_site = sites[0]
    mutated = apply_single_mutation(SAMPLE_SOURCE, ret_site)
    assert mutated is not None
    assert "return None" in mutated


def test_apply_const_mutation() -> None:
    """Applying const_replace on 0 should produce 1."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"const_replace"})
    zero_site = next(s for s in sites if "0 -> 1" in s.description)
    mutated = apply_single_mutation(SAMPLE_SOURCE, zero_site)
    assert mutated is not None
    # Verify the mutation was applied (the source should differ).
    assert mutated != ast.unparse(ast.parse(SAMPLE_SOURCE))


def test_mutation_is_single_site() -> None:
    """Each mutation should only change one site, not the whole file."""
    sites = discover_mutations("<test>", SAMPLE_SOURCE, {"arith_op"})
    if len(sites) < 1:
        return
    site = sites[0]
    mutated = apply_single_mutation(SAMPLE_SOURCE, site)
    assert mutated is not None
    # Parse both and count differences in BinOp nodes.
    orig_tree = ast.parse(SAMPLE_SOURCE)
    mut_tree = ast.parse(mutated)
    orig_ops = [
        type(n.op).__name__ for n in ast.walk(orig_tree) if isinstance(n, ast.BinOp)
    ]
    mut_ops = [
        type(n.op).__name__ for n in ast.walk(mut_tree) if isinstance(n, ast.BinOp)
    ]
    # At most one BinOp should differ.
    diffs = sum(1 for a, b in zip(orig_ops, mut_ops) if a != b)
    assert diffs <= 1, f"Expected at most 1 BinOp change, got {diffs}"


# ---------------------------------------------------------------------------
# Workspace isolation test
# ---------------------------------------------------------------------------


def test_workspace_does_not_modify_original() -> None:
    """create_mutant_workspace must never touch the real source tree."""
    import shutil

    src_root = REPO_ROOT / "src"
    # Pick a small file to test with.
    target = src_root / "molt" / "__init__.py"
    if not target.exists():
        # Skip if the file layout is different.
        return

    original_content = target.read_text(encoding="utf-8")
    mutated_content = original_content + "\n# MUTANT MARKER\n"

    workspace = create_mutant_workspace(target, mutated_content)
    try:
        # Verify workspace has the mutation.
        ws_file = workspace / "molt" / "__init__.py"
        assert ws_file.exists()
        assert "MUTANT MARKER" in ws_file.read_text(encoding="utf-8")

        # Verify original is untouched.
        assert target.read_text(encoding="utf-8") == original_content
    finally:
        shutil.rmtree(workspace, ignore_errors=True)


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------


def test_syntax_error_source_returns_empty() -> None:
    """Files with syntax errors should return no sites."""
    sites = discover_mutations("<bad>", "def foo(\n", {"arith_op"})
    assert sites == []


def test_apply_to_syntax_error_returns_none() -> None:
    """Applying a mutation to unparseable source returns None."""
    site = MutationSite(
        file="<bad>",
        lineno=1,
        col_offset=0,
        operator="arith_op",
        description="Add -> Sub",
        node_index=0,
    )
    result = apply_single_mutation("def foo(\n", site)
    assert result is None


def test_empty_source() -> None:
    """Empty source should return no mutation sites."""
    sites = discover_mutations("<empty>", "", {"arith_op"})
    assert sites == []


# ---------------------------------------------------------------------------
# Known-mutation kill verification
# ---------------------------------------------------------------------------


def test_known_mutation_is_detectable() -> None:
    """A mutation to a simple arithmetic program MUST change its output.

    This is the core correctness property: if we mutate ``a + b`` to
    ``a - b``, the output of ``print(add(3, 4))`` must change from
    ``7`` to ``-1``.

    We use subprocess to execute the code safely in a child process
    rather than running arbitrary code in the test process.
    """
    import subprocess
    import tempfile
    import os

    original = textwrap.dedent("""\
        def add(a, b):
            return a + b
        print(add(3, 4))
    """)

    sites = discover_mutations("<test>", original, {"arith_op"})
    add_site = next(s for s in sites if "Add -> Sub" in s.description)
    mutated = apply_single_mutation(original, add_site)
    assert mutated is not None

    # Write both to temp files and execute via subprocess.
    orig_fd, orig_path = tempfile.mkstemp(suffix=".py")
    mut_fd, mut_path = tempfile.mkstemp(suffix=".py")
    try:
        os.write(orig_fd, original.encode())
        os.close(orig_fd)
        os.write(mut_fd, mutated.encode())
        os.close(mut_fd)

        orig_result = subprocess.run(
            [sys.executable, orig_path],
            capture_output=True,
            text=True,
            timeout=10,
        )
        mut_result = subprocess.run(
            [sys.executable, mut_path],
            capture_output=True,
            text=True,
            timeout=10,
        )
    finally:
        os.unlink(orig_path)
        os.unlink(mut_path)

    assert orig_result.stdout != mut_result.stdout, (
        f"Mutation was NOT detected: "
        f"original={orig_result.stdout!r}, "
        f"mutated={mut_result.stdout!r}"
    )


if __name__ == "__main__":
    # Simple test runner for standalone execution.
    import traceback

    tests = [
        v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)
    ]
    passed = 0
    failed = 0
    for test_fn in tests:
        name = test_fn.__name__
        try:
            test_fn()
            print(f"  PASS  {name}")
            passed += 1
        except Exception:
            print(f"  FAIL  {name}")
            traceback.print_exc()
            failed += 1

    print(f"\n{passed} passed, {failed} failed")
    sys.exit(1 if failed else 0)
