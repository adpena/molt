"""Purpose: differential coverage for PEP 695 type parameter syntax."""

from typing import Callable


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


events = []


def mark(name, value):
    events.append(name)
    return value


type LaterAlias = mark("alias", LaterValue)
print("alias before", events)
LaterValue = int
print("alias value", LaterAlias.__value__, events)
print("alias cached", LaterAlias.__value__, events)


class AliasOwner:
    type Member = LaterMember
    LaterMember = str


print("class alias", AliasOwner.Member.__value__)


def local_alias_value():
    type LocalAlias = LaterLocal
    LaterLocal = float
    return LocalAlias.__value__


print("local alias", local_alias_value())

attempts = 0


def retry_value():
    global attempts
    attempts += 1
    if attempts == 1:
        raise ValueError("first")
    return bytes


type RetryAlias = retry_value()
for index in range(3):
    try:
        print("retry value", index, RetryAlias.__value__, attempts)
    except Exception as exc:
        print("retry error", index, type(exc).__name__, str(exc), attempts)


type Bounded[T: int, U: (str, bytes)] = tuple[T, U]
print("type param bound", Bounded.__type_params__[0].__bound__)
print("type param constraints", Bounded.__type_params__[1].__constraints__)

bound_attempts = 0


def retry_bound():
    global bound_attempts
    bound_attempts += 1
    if bound_attempts == 1:
        raise ValueError("bound first")
    return memoryview


type RetryBound[T: retry_bound()] = T
retry_parameter = RetryBound.__type_params__[0]
for index in range(3):
    try:
        print("retry bound", index, retry_parameter.__bound__, bound_attempts)
    except Exception as exc:
        print("retry bound error", index, type(exc).__name__, str(exc), bound_attempts)

type Variadic[*Ts] = tuple[*Ts]
type Callback[**P] = Callable[P, int]
print(
    "typevar kind",
    type(Pair.__type_params__[0]).__name__,
    type(Pair.__type_params__[0]).__qualname__,
)
print(
    "typevartuple kind",
    type(Variadic.__type_params__[0]).__name__,
    type(Variadic.__type_params__[0]).__qualname__,
)
print(
    "paramspec kind",
    type(Callback.__type_params__[0]).__name__,
    type(Callback.__type_params__[0]).__qualname__,
)
