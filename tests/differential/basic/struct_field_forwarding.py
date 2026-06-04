"""Purpose: differential coverage for MemGVN store-to-load forwarding (S5-2b).

Exercises the four forwarding scenarios that must stay byte-identical to
CPython after the load is rewritten to a Copy of the stored value:

  1. straight-line instance-attr store -> load forwarding,
  2. the same forward inside a loop (per-iteration store then read),
  3. a function call between store and load (must NOT forward — the call may
     mutate the field; the post-call read must observe the call's effect),
  4. two names aliasing the same object (store through one, read through both),
  5. an int field that exceeds 2 ** 60 (forwarding must stay BigInt-correct —
     the forwarded value inherits the stored value's MaybeBigInt repr, never a
     trusted-unbox).
"""


class P:
    x: int
    y: int

    def __init__(self, x: int = 0, y: int = 0) -> None:
        self.x = x
        self.y = y

    def bump(self) -> None:
        self.x = 99


# 1. Straight-line forward: p.x and p.y read back the constructor stores.
def straight_line() -> int:
    p = P(3, 4)
    return p.x + p.y


# 2. Forward in a loop: each iteration stores then reads the same field.
def loop_forward(n: int) -> int:
    p = P(0, 0)
    acc = 0
    for i in range(n):
        p.x = i
        p.y = i + 1
        acc += p.x + p.y
    return acc


# 3. A call between store and load: the method call may mutate p.x, so the
#    store must NOT be forwarded past it. The read must observe the call's
#    effect (99), not the pre-call store (7).
def call_between() -> int:
    p = P(1, 2)
    p.x = 7
    p.bump()  # opaque CallMethod — clobbers p.x
    return p.x  # must read 99, not 7


# 4. Two names, same object: store through `q`, read through both `p` and `q`.
def aliased() -> int:
    p = P(5, 6)
    q = p
    q.x = 10
    return p.x + q.x  # 10 + 10 == 20


# 5. BigInt safety: a field value >= 2 ** 60 must round-trip exactly.
def bigint_field(v: int) -> int:
    n = P(v, v + 1)
    return n.x + n.y


print(straight_line())
print(loop_forward(5))
print(call_between())
print(aliased())
print(bigint_field(1 << 60))
print(bigint_field((1 << 80) + 123))

assert straight_line() == 7
assert loop_forward(5) == sum(i + (i + 1) for i in range(5))
assert call_between() == 99
assert aliased() == 20
assert bigint_field(1 << 60) == (1 << 60) + (1 << 60) + 1
