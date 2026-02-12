"""Purpose: regress builtin name resolution for locals/__import__ and builtins exports."""

imp = __import__
print(callable(imp))
print(imp is __import__)
print(imp("builtins").__name__)

loc_fn = locals
print(callable(loc_fn))


def f():
    alias = locals
    d = alias()
    print(isinstance(d, dict))
    print(alias is __import__("builtins").locals)


f()

builtins = __import__("builtins")

print(hasattr(builtins, "globals"))
print(hasattr(builtins, "locals"))
print(isinstance(builtins.globals(), dict))
print("__name__" in builtins.globals())
print(isinstance(builtins.locals(), dict))
