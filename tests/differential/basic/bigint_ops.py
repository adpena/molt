"""Purpose: differential coverage for bigint ops."""

a = 1 << 60
b = a + 123
literal = 123456789012345678901234567890

print(a)
print(b)
print(literal)
print(a + b)
print(a * 3)
print(a // 7)
print(a % 7)
print(a << 5)
print(a >> 3)
print(a | b)
print(int("123456789012345678901234567890"))
print(int(1e20))
print(round(1e20))
