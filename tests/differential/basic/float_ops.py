print(float(), float(1), float(True))
print(float(" 2.5 "), float(b"3.5"))

print(1.0 + 2.5, 5.0 - 2.0, 2.0 * 3.5)
print(5 / 2, 5 // 2, 5 % 2)
print(5.0 // 2.0, 5.0 % 2.0)
print(2**3, 2.0**3, 2**3.0, 2.0**3.0)
print(1.0 == 1, 1.0 == 2, 1.0 < 2, 2.0 <= 2, 3.0 > 2, 3.0 >= 4)

nan = float("nan")
print(nan == nan, nan < 1, nan > 1)
print(float("inf"), float("-inf"))
