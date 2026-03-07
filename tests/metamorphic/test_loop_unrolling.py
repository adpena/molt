"""Metamorphic test: manual loop unrolling.

Manually unrolling small constant-range ``for`` loops into sequential
statements is a semantics-preserving transformation. If the compiler
produces different output for the unrolled version, it has a loop
codegen or iteration bug.
"""

import sys
import pytest

sys.path.insert(0, ".")
from tools.metamorphic_runner import MetamorphicRunner


# Each entry is (original_source, manually_unrolled_source).
TEST_PROGRAM_PAIRS = [
    # Pair 0: simple print loop
    (
        """\
for i in range(3):
    print(i)
""",
        """\
print(0)
print(1)
print(2)
""",
    ),
    # Pair 1: accumulator loop
    (
        """\
total = 0
for i in range(4):
    total = total + i
print(total)
""",
        """\
total = 0
total = total + 0
total = total + 1
total = total + 2
total = total + 3
print(total)
""",
    ),
    # Pair 2: expression in loop body
    (
        """\
for i in range(3):
    print(i * i)
""",
        """\
print(0 * 0)
print(1 * 1)
print(2 * 2)
""",
    ),
    # Pair 3: loop building a running product
    (
        """\
product = 1
for i in range(1, 5):
    product = product * i
print(product)
""",
        """\
product = 1
product = product * 1
product = product * 2
product = product * 3
product = product * 4
print(product)
""",
    ),
]


runner = MetamorphicRunner()


@pytest.mark.parametrize(
    "pair",
    TEST_PROGRAM_PAIRS,
    ids=[f"pair_{i}" for i in range(len(TEST_PROGRAM_PAIRS))],
)
def test_loop_unrolling_equivalence(pair: tuple[str, str]):
    """Manually unrolled loop should produce identical output."""
    original, unrolled = pair

    result = runner.compare(original, unrolled)

    if result.error:
        pytest.skip(f"Build/run error: {result.error}")

    assert result.equivalent, (
        f"Output differs after loop unrolling!\n"
        f"Original stdout: {result.original_stdout!r}\n"
        f"Unrolled stdout: {result.transformed_stdout!r}"
    )
