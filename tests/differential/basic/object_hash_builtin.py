"""Purpose: ensure object.__hash__ uses identity semantics without recursion."""


class C:
    pass


obj = C()
print(isinstance(object.__hash__(obj), int))
print(hash(obj) == object.__hash__(obj))
print(isinstance(object.__hash__([]), int))
