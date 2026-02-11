"""Ensure inline-int overflow promotes instead of wrapping."""

a = (1 << 46) - 1
print(f"a0:{a}")
a = a + 1
print(f"a1:{a}")

b = (1 << 46) - 2
b += 5
print(f"b1:{b}")

c = (1 << 46) - 4
for _ in range(10):
    c = c + 1
print(f"c1:{c}")
