# Generated from Lean theorem: evalLuauExpr_nil
# Source: formal/lean/MoltTIR/Backend/LuauSemantics.lean
# Property: None literal evaluates correctly.

print(None)
print(type(None).__name__)
print(None is None)
print(None == None)
print(bool(None))

# None is a singleton
a = None
b = None
print(a is b)

# None is falsy
if not None:
    print("None is falsy")
else:
    print("None is truthy")

# repr and str
print(repr(None))
print(str(None))
