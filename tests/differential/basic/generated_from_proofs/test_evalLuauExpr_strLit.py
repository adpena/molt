# Generated from Lean theorem: evalLuauExpr_strLit
# Source: formal/lean/MoltTIR/Backend/LuauSemantics.lean
# Property: String literals evaluate to their expected values.

print("")
print("hello")
print("world")
print("hello world")
print(type("").__name__)
print(type("hello").__name__)

# Empty string is falsy
print(bool(""))
print(bool("x"))

# Length
print(len(""))
print(len("hello"))
print(len("abc"))

# Escape sequences
print("a\nb")
print("a\tb")
print("a\\b")
