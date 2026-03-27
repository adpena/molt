"""Purpose: stress builtin __import__ on the canonical top-level import path."""


math_mod = __import__("math")

print(math_mod.__name__)
print(math_mod is __import__("math"))
