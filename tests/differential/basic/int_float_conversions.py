"""Purpose: differential coverage for int float conversions."""

print(int("0b101", 0))
print(int("0o10", 0))
print(int("0x10", 0))

nan = float("nan")
print(nan == nan)
print(nan != nan)
