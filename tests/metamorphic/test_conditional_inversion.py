"""Metamorphic test: conditional branch inversion.

Inverting an if/else condition and swapping the branches is a
semantics-preserving transformation:

    if COND: A else: B  ≡  if not COND: B else: A

If the compiler produces different output for the inverted version,
it has a conditional codegen or boolean logic bug.
"""

import sys
import pytest

sys.path.insert(0, ".")
from tools.metamorphic_runner import MetamorphicRunner


# Each entry is (original_source, inverted_source).
TEST_PROGRAM_PAIRS = [
    # Pair 0: greater-than → not-greater-than
    (
        """\
x = 10
if x > 5:
    print("big")
else:
    print("small")
""",
        """\
x = 10
if not (x > 5):
    print("small")
else:
    print("big")
""",
    ),
    # Pair 1: equality → inequality
    (
        """\
a = 3
b = 3
if a == b:
    print("equal")
else:
    print("different")
""",
        """\
a = 3
b = 3
if a != b:
    print("different")
else:
    print("equal")
""",
    ),
    # Pair 2: less-than with arithmetic in branches
    (
        """\
val = 7
if val < 10:
    result = val * 2
else:
    result = val + 1
print(result)
""",
        """\
val = 7
if not (val < 10):
    result = val + 1
else:
    result = val * 2
print(result)
""",
    ),
    # Pair 3: nested conditional inversion (outer only)
    (
        """\
x = 4
y = 8
if x + y > 10:
    if x > 3:
        print("both")
    else:
        print("outer only")
else:
    print("neither")
""",
        """\
x = 4
y = 8
if not (x + y > 10):
    print("neither")
else:
    if x > 3:
        print("both")
    else:
        print("outer only")
""",
    ),
]


runner = MetamorphicRunner()


@pytest.mark.parametrize(
    "pair",
    TEST_PROGRAM_PAIRS,
    ids=[f"pair_{i}" for i in range(len(TEST_PROGRAM_PAIRS))],
)
def test_conditional_inversion_equivalence(pair: tuple[str, str]):
    """Inverted conditional with swapped branches should produce identical output."""
    original, inverted = pair

    result = runner.compare(original, inverted)

    if result.error:
        pytest.skip(f"Build/run error: {result.error}")

    assert result.equivalent, (
        f"Output differs after conditional inversion!\n"
        f"Original stdout: {result.original_stdout!r}\n"
        f"Inverted stdout: {result.transformed_stdout!r}"
    )
