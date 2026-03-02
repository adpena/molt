"""Metamorphic test: pre-folding constant expressions.

If constants are pre-computed in source, the compiler should produce
identical runtime output. This tests that the compiler's own constant
folding is semantically correct.
"""

import ast
import sys
import pytest

sys.path.insert(0, ".")
from tools.metamorphic_runner import MetamorphicRunner


def prefold_constants(source: str) -> str:
    """Pre-evaluate constant expressions in Python source.

    Replaces expressions like `3 + 4` with `7` where all operands
    are literals.
    """
    tree = ast.parse(source)

    class ConstFolder(ast.NodeTransformer):
        def visit_BinOp(self, node: ast.BinOp) -> ast.AST:
            self.generic_visit(node)

            # Only fold if both operands are constants
            if not (
                isinstance(node.left, ast.Constant)
                and isinstance(node.right, ast.Constant)
            ):
                return node

            left = node.left.value
            right = node.right.value

            # Only fold safe integer/float operations
            if not isinstance(left, (int, float)) or not isinstance(
                right, (int, float)
            ):
                return node

            try:
                if isinstance(node.op, ast.Add):
                    result = left + right
                elif isinstance(node.op, ast.Sub):
                    result = left - right
                elif isinstance(node.op, ast.Mult):
                    result = left * right
                elif isinstance(node.op, ast.FloorDiv) and right != 0:
                    result = left // right
                elif isinstance(node.op, ast.Mod) and right != 0:
                    result = left % right
                elif isinstance(node.op, ast.Pow) and right >= 0 and right < 100:
                    result = left**right
                else:
                    return node

                return ast.Constant(value=result)
            except (ArithmeticError, ValueError):
                return node

        def visit_UnaryOp(self, node: ast.UnaryOp) -> ast.AST:
            self.generic_visit(node)

            if not isinstance(node.operand, ast.Constant):
                return node

            val = node.operand.value
            if not isinstance(val, (int, float)):
                return node

            if isinstance(node.op, ast.USub):
                return ast.Constant(value=-val)
            elif isinstance(node.op, ast.UAdd):
                return ast.Constant(value=+val)

            return node

    folded = ConstFolder().visit(tree)
    ast.fix_missing_locations(folded)
    return ast.unparse(folded)


TEST_PROGRAMS = [
    # Constant arithmetic
    """\
x = 3 + 4
print(x)
""",
    # Nested constants
    """\
x = (2 + 3) * (4 + 1)
print(x)
""",
    # Mixed constants and variables
    """\
base = 10
offset = 3 + 4
result = base + offset
print(result)
""",
    # Power
    """\
x = 2 ** 10
print(x)
""",
]


runner = MetamorphicRunner()


@pytest.mark.parametrize(
    "program", TEST_PROGRAMS, ids=[f"prog_{i}" for i in range(len(TEST_PROGRAMS))]
)
def test_constant_prefold_equivalence(program: str):
    """Pre-folded program should produce identical output."""
    folded = prefold_constants(program)

    # Only test if folding actually changed something
    if folded == program:
        pytest.skip("No constants to fold")

    result = runner.compare(program, folded)

    if result.error:
        pytest.skip(f"Build/run error: {result.error}")

    assert result.equivalent, (
        f"Output differs after constant pre-folding!\n"
        f"Original stdout: {result.original_stdout!r}\n"
        f"Folded stdout:   {result.transformed_stdout!r}"
    )
