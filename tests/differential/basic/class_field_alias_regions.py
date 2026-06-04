# Class-aware TypedField alias regions (S5-1.5): repeated field loads on the
# same object across distinct offsets, exception-bearing method bodies, and a
# >= 2**60 bigint field that MUST stay BigInt-correct (the typed-slot region
# must never enable a trusted-unbox of a heap BigInt). Output must be
# byte-identical to CPython 3.14.


class Vec3:
    def __init__(self, x, y, z):
        self.x = x
        self.y = y
        self.z = z

    def dot(self, other):
        # Two reads each of self.x/self.y/self.z + other.x/other.y/other.z.
        # Distinct offsets within the same class are disjoint fields; the
        # repeated reads of the SAME offset are MemGVN-forwardable.
        return (
            self.x * other.x
            + self.y * other.y
            + self.z * other.z
        )

    def norm_sq(self):
        # self.x read twice, self.y twice, self.z twice — three forwardable
        # same-offset load pairs on one object.
        return self.x * self.x + self.y * self.y + self.z * self.z

    def scaled_sum(self, k):
        # An exception-bearing body: division by a possibly-zero argument sits
        # between repeated field reads. The class-version guard / CheckException
        # must not let a field load be forwarded across the raise incorrectly.
        try:
            inv = self.x // k
        except ZeroDivisionError:
            inv = 0
        return inv + self.y + self.z + self.x


class BigBox:
    def __init__(self, base):
        self.lo = base
        self.hi = base + 1

    def widen(self, shift):
        # self.lo read twice, self.hi twice. The values are >= 2**60 BigInts:
        # the TypedField region must NOT permit any trusted i64 unbox of the
        # boxed BigInt slot.
        return (self.lo << shift) + (self.hi << shift) + self.lo - self.hi


def main():
    a = Vec3(3, 4, 5)
    b = Vec3(10, 20, 30)
    print(a.dot(b))
    print(a.norm_sq())
    print(b.norm_sq())
    print(a.scaled_sum(2))
    print(a.scaled_sum(0))

    big = BigBox(1 << 60)
    print(big.lo)
    print(big.hi)
    print(big.widen(7))
    print(big.widen(0))

    # A loop reading the same field repeatedly (the forwardable shape inside a
    # hot loop).
    acc = 0
    v = Vec3(2, 3, 4)
    for _ in range(5):
        acc = acc + v.x + v.x + v.y
    print(acc)


main()
