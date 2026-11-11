"""The primitive instruction programs: one declaration of workload bytes.

Each program descends from the integrator's 2026-10-31 copy primitive
experiment or 2026-11-01 construction experiment. N is supplied at execution,
never at compilation. Counts include loop/control work; no empty-loop
subtraction. tools/perf_instructions.py measures them; its per-op axes are the
HF-459 axes the V1 ledger's engine families own (tools/finding_coverage.py).
This module imports nothing heavier than dataclasses, so ledger tooling can
read the axis list without the measurement stack.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Primitive:
    name: str
    body: str
    iterations: int = 1_000_000
    divisor: int = 1
    setup: str = ""
    result: str = "run(n)"

    def program(self) -> str:
        return (
            "import sys\n"
            + self.setup
            + "\ndef run(n):\n"
            + "\n".join("    " + line for line in self.body.splitlines())
            + "\n\nn = int(sys.argv[1])\nprint("
            + self.result
            + ")\n"
        )

    def operations(self) -> int:
        return self.iterations // self.divisor


_COPY_SETUP = """
class Plain:
    def __init__(self):
        self.a = 1

def two_args(x, memo):
    return x

DISPATCH = {int: two_args, str: two_args, Plain: two_args}
OBJS = [Plain() for _ in range(64)]
NIL = []
"""

_ID_ORACLE = """
def check_checksum(total, n):
    ids = [id(obj) for obj in OBJS]
    if len(set(ids)) != len(OBJS):
        return False
    expected = 0
    # The 64-object cycle cancels after two passes. Calculate the reference
    # outside the measured loop, retaining the full loop result as an input.
    for i in range(n % 128):
        expected ^= ids[i & 63]
    return total == expected
"""

# Process start alone; its startup axis owns every program's fixed cost.
EMPTY = Primitive("empty", "return n", iterations=1)

# One declaration owns the complete set, workload bytes and normalization.
# reduce_ex/new_and_update count outer operations, recursion counts a depth-100
# traversal. Each legacy loop retains its original body and loop divisor.
PRIMITIVES = (
    Primitive(
        "loop",
        "total = 0\nfor i in range(n):\n    total += i\nreturn total",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "id",
        "total = 0\nobjs = OBJS\nfor i in range(n):\n    total ^= id(objs[i & 63])\nreturn total",
        setup=_COPY_SETUP + _ID_ORACLE,
        result="check_checksum(run(n), n)",
    ),
    Primitive(
        "memo_get",
        "memo = {id(o): o for o in OBJS}\nobjs = OBJS\nhits = 0\nfor i in range(n):\n    if memo.get(id(objs[i & 63]), NIL) is not NIL:\n        hits += 1\nreturn hits",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "memo_set",
        "memo = {}\nobjs = OBJS\nfor i in range(n):\n    memo[id(objs[i & 63])] = i\nreturn len(memo)",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "type_dispatch",
        "objs = OBJS\nfound = 0\nfor i in range(n):\n    if DISPATCH.get(type(objs[i & 63])) is not None:\n        found += 1\nreturn found",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "getattr_miss",
        'objs = OBJS\nmisses = 0\nfor i in range(n):\n    if getattr(objs[i & 63], "__deepcopy__", None) is None:\n        misses += 1\nreturn misses',
        setup=_COPY_SETUP,
    ),
    Primitive(
        "dynamic_call",
        "f = DISPATCH[int]\ntotal = 0\nfor i in range(n):\n    total += f(i, None)\nreturn total",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "isinstance",
        "objs = OBJS\ncount = 0\nfor i in range(n):\n    if isinstance(objs[i & 63], str):\n        count += 1\nreturn count",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "tuple_unpack",
        "total = 0\nfor i in range(n):\n    rv = (i, None, i, None, None)\n    a, b, c, d, e = rv\n    total += a + c\nreturn total",
        setup=_COPY_SETUP,
    ),
    Primitive(
        "reduce_ex",
        "objs = OBJS\ncount = 0\nfor i in range(n // 10):\n    count += len(objs[i & 63].__reduce_ex__(4))\nreturn count",
        divisor=10,
        setup=_COPY_SETUP,
    ),
    Primitive(
        "new_and_update",
        'state = {"a": 1}\ncount = 0\nfor i in range(n // 10):\n    y = Plain.__new__(Plain)\n    y.__dict__.update(state)\n    count += y.a\nreturn count',
        divisor=10,
        setup=_COPY_SETUP,
    ),
    Primitive(
        "recursion",
        "total = 0\nfor _ in range(n // 100):\n    total += recurse(100)\nreturn total",
        divisor=100,
        setup=_COPY_SETUP
        + "\ndef recurse(n):\n    if n == 0:\n        return 0\n    return recurse(n - 1) + 1\n",
    ),
    Primitive(
        "plain_construction",
        "p = Point(-1, 2)\nfor i in range(n):\n    p = Point(i, 2)\nreturn p",
        iterations=100_000,
        setup="\nclass Point:\n    def __init__(self, x, y):\n        self.x = x\n        self.y = y\n",
        result="(lambda p: (p.x, p.y))(run(n))",
    ),
    Primitive(
        "dataclass_construction",
        "p = Point(-1, 2)\nfor i in range(n):\n    p = Point(i, 2)\nreturn p",
        iterations=100_000,
        setup="\nfrom dataclasses import dataclass\n\n@dataclass\nclass Point:\n    x: int\n    y: int\n",
        result="(lambda p: (p.x, p.y))(run(n))",
    ),
    Primitive(
        "raise_user",
        'count = 0\nfor _ in range(n):\n    try:\n        raise ValueError("x")\n    except ValueError:\n        count += 1\nreturn count',
    ),
    Primitive(
        "raise_runtime",
        'count = 0\nfor _ in range(n):\n    try:\n        int("abc")\n    except ValueError:\n        count += 1\nreturn count',
    ),
)


def per_op_axes() -> tuple[str, ...]:
    """Every per-operation axis; each names one primitive's cost (HF-459)."""
    return tuple(f"{p.name}/per_op" for p in PRIMITIVES)
