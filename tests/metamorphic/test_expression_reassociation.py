"""Metamorphic test: expression reassociation.

Reordering operands of associative and commutative integer operations
(addition, multiplication) is a semantics-preserving transformation.
This is only valid for integers — floating-point reassociation can
change results due to precision — so all test programs use integer-only
arithmetic.

If the compiler produces different output for the reassociated version,
it has an expression codegen or evaluation-order bug.
"""

import sys
import pytest

sys.path.insert(0, ".")
from tools.metamorphic_runner import MetamorphicRunner


# Each entry is (original_source, reassociated_source).
TEST_PROGRAM_PAIRS = [
    # Pair 0: addition commutativity — a + b + c → c + a + b
    (
        """\
a = 3
b = 7
c = 11
result = a + b + c
print(result)
""",
        """\
a = 3
b = 7
c = 11
result = c + a + b
print(result)
""",
    ),
    # Pair 1: multiplication commutativity — a * b * c → c * b * a
    (
        """\
a = 2
b = 5
c = 8
result = a * b * c
print(result)
""",
        """\
a = 2
b = 5
c = 8
result = c * b * a
print(result)
""",
    ),
    # Pair 2: mixed operations — (a + b) * c == (b + a) * c
    (
        """\
a = 4
b = 6
c = 3
result = (a + b) * c
print(result)
""",
        """\
a = 4
b = 6
c = 3
result = (b + a) * c
print(result)
""",
    ),
    # Pair 3: chained addition in a loop accumulator
    (
        """\
x = 10
y = 20
z = 30
total = x + y + z + 1
print(total)
""",
        """\
x = 10
y = 20
z = 30
total = 1 + z + y + x
print(total)
""",
    ),
]


runner = MetamorphicRunner()


@pytest.mark.parametrize(
    "pair",
    TEST_PROGRAM_PAIRS,
    ids=[f"pair_{i}" for i in range(len(TEST_PROGRAM_PAIRS))],
)
def test_expression_reassociation_equivalence(pair: tuple[str, str]):
    """Reassociated integer expression should produce identical output."""
    original, reassociated = pair

    result = runner.compare(original, reassociated)

    if result.error:
        pytest.skip(f"Build/run error: {result.error}")

    assert result.equivalent, (
        f"Output differs after expression reassociation!\n"
        f"Original stdout:      {result.original_stdout!r}\n"
        f"Reassociated stdout:  {result.transformed_stdout!r}"
    )
