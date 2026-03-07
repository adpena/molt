"""Metamorphic test: dead code insertion.

Inserting unreachable branches, unused variables, and no-op statements
should not affect program output. If the compiler produces different
results after dead code is added, it has a code elimination or
control-flow bug.
"""

import sys
import textwrap
import pytest

sys.path.insert(0, ".")
from tools.metamorphic_runner import MetamorphicRunner


def inject_dead_code(source: str) -> str:
    """Inject various forms of dead code into a Python program.

    Inserts unreachable ``if False`` blocks, unused variable assignments,
    and ``pass`` statements into the source without changing its semantics.
    The injected code is placed at the module level before the real program
    body so it cannot interfere with control flow.
    """
    dead_preamble = textwrap.dedent("""\
        if False:
            print("this is dead code and should never execute")
            _unreachable = 999
        _dead_var_1 = 42
        _dead_var_2 = "unused"
        pass
    """)
    return dead_preamble + source


TEST_PROGRAMS = [
    # Simple print with arithmetic
    """\
x = 10
y = 20
print(x + y)
""",
    # Loop with accumulator
    """\
total = 0
for i in range(5):
    total = total + i
print(total)
""",
    # Conditional branches (dead code should not shadow these)
    """\
x = 7
if x > 5:
    print("big")
else:
    print("small")
""",
    # Function definition and call
    """\
def square(n):
    return n * n

result = square(6)
print(result)
""",
]


runner = MetamorphicRunner()


@pytest.mark.parametrize(
    "program", TEST_PROGRAMS, ids=[f"prog_{i}" for i in range(len(TEST_PROGRAMS))]
)
def test_dead_code_insertion_equivalence(program: str):
    """Program with injected dead code should produce identical output."""
    transformed = inject_dead_code(program)
    assert transformed != program, "Dead code injection should change source"

    result = runner.compare(program, transformed)

    if result.error:
        pytest.skip(f"Build/run error: {result.error}")

    assert result.equivalent, (
        f"Output differs after dead code insertion!\n"
        f"Original stdout: {result.original_stdout!r}\n"
        f"Dead-code stdout: {result.transformed_stdout!r}"
    )
