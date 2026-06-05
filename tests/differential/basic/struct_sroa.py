"""Purpose: differential coverage for SROA — scalar replacement of aggregates
(S5-2d). A proven-non-escaping object whose fields are only written (and read
through MemGVN-forwarded loads) must compile to register moves: SROA removes the
stores, DCE removes the allocation. Output must be byte-identical to CPython 3.14.

Each case stresses a distinct SROA precondition. SROA is fail-closed: a refused
promotion is a missed optimization, never a miscompile — so every case must
produce the SAME result whether or not SROA fired.

  1. bench_struct pattern — object constructed + mutated in a hot loop, read back
     each iteration (the loads forward, the stores+alloc vanish);
  2. inline-int field values across a loop (fits the 47-bit window → SROA-safe);
  3. a >= 2**60 BigInt FIELD — the store value is a heap pointer, so SROA MUST
     refuse to remove the store (removing it would unbalance the slot capture);
     the result must stay BigInt-correct;
  4. a >= 2**63 ACCUMULATOR fed by a field — the accumulator overflows the inline
     window and MUST stay a boxed BigInt.

Exception-bearing field bodies (try/except around field access) are covered by
`class_field_alias_regions.py` (method-receiver objects); SROA correctly refuses
to promote any object whose fields are observed across an exception boundary
(the surviving post-try load is a blocker), so those cases exercise the
fail-closed refusal path rather than a transform.
"""


class Point:
    x: int
    y: int

    def __init__(self, x: int = 0, y: int = 0) -> None:
        self.x = x
        self.y = y


# 1 + 2. The doc's bench_struct proving ground: build a fresh Point each
# iteration, write inline-int fields, read them back. After MemGVN forwards the
# reads, the object is store-only → SROA removes the stores, DCE the alloc.
def sroa_loop() -> int:
    total = 0
    for i in range(10):
        p = Point(0, 0)
        p.x = i
        p.y = i + 1
        total += p.x + p.y
    return total


# 3. A >= 2**60 BigInt field value: the store captures a heap pointer, so SROA
#    must REFUSE to remove the store. The read still forwards (MemGVN), so the
#    result is correct either way — and it must stay an exact BigInt.
def bigint_field(v: int) -> int:
    p = Point(v, v + 1)
    return p.x + p.y


# 4. A >= 2**63 accumulator fed by inline-int fields. The accumulator itself
#    overflows the inline window and must carry as a boxed BigInt.
def bigint_accumulator(n: int) -> int:
    total = 1 << 62
    for i in range(n):
        p = Point(i, i)
        total += p.x + p.y
    return total


print(sroa_loop())
print(bigint_field(1 << 60))
print(bigint_field((1 << 80) + 5))
print(bigint_accumulator(4))

assert sroa_loop() == sum(i + (i + 1) for i in range(10))
assert bigint_field(1 << 60) == (1 << 60) + (1 << 60) + 1
assert bigint_accumulator(4) == (1 << 62) + sum(i + i for i in range(4))
