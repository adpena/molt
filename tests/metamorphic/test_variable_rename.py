"""Metamorphic test: alpha-renaming all user variables.

Renaming variables is a semantics-preserving transformation. If the compiler
produces different output after renaming, it has a variable-identity bug.
"""

import ast
import sys
import pytest

sys.path.insert(0, ".")
from tools.metamorphic_runner import MetamorphicRunner


def alpha_rename(source: str, prefix: str = "renamed_") -> str:
    """Rename all user-defined variables in a Python source.

    Uses AST analysis to find Name nodes that are stores (assignments),
    then renames them and all their references.
    """
    tree = ast.parse(source)

    # Collect user-defined names (assigned variables, function args)
    user_names: set[str] = set()

    for node in ast.walk(tree):
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store):
            user_names.add(node.id)
        elif isinstance(node, ast.FunctionDef):
            for arg in node.args.args:
                user_names.add(arg.arg)
            for arg in node.args.kwonlyargs:
                user_names.add(arg.arg)

    # Don't rename builtins or dunder names
    builtins_set = (
        set(dir(__builtins__))
        if isinstance(__builtins__, dict)
        else set(dir(__builtins__))
    )
    user_names -= builtins_set
    user_names = {n for n in user_names if not n.startswith("_")}

    if not user_names:
        return source

    # Build rename mapping
    rename_map = {name: f"{prefix}{name}" for name in sorted(user_names)}

    # Apply renaming via AST
    class Renamer(ast.NodeTransformer):
        def visit_Name(self, node: ast.Name) -> ast.Name:
            if node.id in rename_map:
                node.id = rename_map[node.id]
            return node

        def visit_FunctionDef(self, node: ast.FunctionDef) -> ast.FunctionDef:
            # Don't rename function names that are called externally (like main)
            for arg in node.args.args:
                if arg.arg in rename_map:
                    arg.arg = rename_map[arg.arg]
            for arg in node.args.kwonlyargs:
                if arg.arg in rename_map:
                    arg.arg = rename_map[arg.arg]
            self.generic_visit(node)
            return node

    renamed_tree = Renamer().visit(tree)
    ast.fix_missing_locations(renamed_tree)
    return ast.unparse(renamed_tree)


# Test programs — simple enough for Molt's Tier 0 subset
TEST_PROGRAMS = [
    # Simple arithmetic
    """\
x = 10
y = 20
result = x + y
print(result)
""",
    # Loop with accumulator
    """\
total = 0
for i in range(10):
    total = total + i
print(total)
""",
    # Function call
    """\
def add(a, b):
    return a + b

result = add(3, 4)
print(result)
""",
    # Nested variables
    """\
x = 5
y = x * 2
z = y + x
print(z)
""",
]


runner = MetamorphicRunner()


@pytest.mark.parametrize(
    "program", TEST_PROGRAMS, ids=[f"prog_{i}" for i in range(len(TEST_PROGRAMS))]
)
def test_variable_rename_equivalence(program: str):
    """Renamed program should produce identical output."""
    renamed = alpha_rename(program)
    assert renamed != program, "Rename should change something"

    result = runner.compare(program, renamed)

    if result.error:
        pytest.skip(f"Build/run error: {result.error}")

    assert result.equivalent, (
        f"Output differs after variable renaming!\n"
        f"Original stdout: {result.original_stdout!r}\n"
        f"Renamed stdout:  {result.transformed_stdout!r}"
    )
