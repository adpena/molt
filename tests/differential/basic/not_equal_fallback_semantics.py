"""Purpose: __ne__ fallback respects value semantics, not object-bit identity."""

s1 = "molt_meta_target"
s2 = "".join(["molt_", "meta_", "target"])

print(s1 == s2)
print(s1 != s2)
print("x" != "x")
print("x" != "y")

class NoCmp:
    pass

a = NoCmp()
b = NoCmp()
print(a != a)
print(a != b)
