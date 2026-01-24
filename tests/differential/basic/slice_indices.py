"""Purpose: differential coverage for slice indices."""


class BadCmp:
    def __eq__(self, other):
        raise ValueError("boom")


s = slice(1, 2, 3)
print(f"members:{s.start},{s.stop},{s.step}")
print(f"indices_basic:{slice(None).indices(5)}")
print(f"indices_step:{slice(1, None, 2).indices(10)}")
print(f"indices_neg:{slice(None, None, -1).indices(4)}")
print(f"indices_big:{slice(None, None, -1).indices(2**80)}")

print(f"eq_true:{slice(1, 2, 3) == slice(1, 2, 3)}")
print(f"eq_false:{slice(1, 2, 3) == slice(1, 2, 4)}")
print(f"eq_other:{slice(1, 2, 3) == (1, 2, 3)}")

try:
    _ = slice(BadCmp()) == slice(BadCmp())
except Exception as exc:
    print(f"eq_err:{type(exc).__name__}:{exc}")

print(f"hash:{hash(slice(1, 2, 3))}")
try:
    hash(slice(1, 2, []))
except Exception as exc:
    print(f"hash_err:{type(exc).__name__}:{exc}")
