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



# A heap integer literal remains one Python object when a list repeats it.
def heap_literal_fill(count):
    values = [4611686018427387904] * count
    return len(values), values[0], values[0] is values[-1]


# Each evaluation of this code constant publishes the same object reference.
def heap_constant_comprehension():
    values = [4611686018427387904 for unused in range(3)]
    return len(values), values[0], values[0] is values[-1]


print("heap literal fill", heap_literal_fill(3))
print("heap constant comprehension", heap_constant_comprehension())
