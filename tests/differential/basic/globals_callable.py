"""Purpose: globals() as a first-class callable."""

g = globals
print(g is globals)
print(g() is globals())
print("__name__" in g())
