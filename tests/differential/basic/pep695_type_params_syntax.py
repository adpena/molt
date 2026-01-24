"""Purpose: differential coverage for PEP 695 type parameter syntax."""


class Box[T]:
    def __init__(self, value: T) -> None:
        self.value = value


def ident[T](value: T) -> T:
    return value


type Pair[T] = tuple[T, T]


boxed = Box[int](1)
print(boxed.value)
print(ident("ok"))
print(Pair[int])
print(getattr(Box, "__type_params__", None))
print(getattr(ident, "__type_params__", None))
