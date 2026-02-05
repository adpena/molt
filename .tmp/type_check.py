T = type.__new__(type, "X", (tuple,), {})
print(isinstance(T, type), issubclass(T, tuple))
print(T(1, 2, 3))
