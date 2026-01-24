"""Purpose: differential coverage for str count unicode slices."""

s = "cafe\u0301"

print(s.count("é"))
print(s.count("e\u0301"))
print(s.count("e\u0301", 0, 4))
print(s.count("e\u0301", 3, 5))
print(s.count("e\u0301", -2, len(s)))
print(s.count("", 0, 2))
