"""Purpose: differential coverage for typing basics used by Click/Trio."""

from typing import Any, Callable, Generic, Literal, TypedDict, TypeVar

T = TypeVar("T")


class Box(Generic[T]):
    def __init__(self, value: T):
        self.value = value


class Config(TypedDict):
    name: str
    count: int


def accepts(value: Literal["a", "b"]):
    return value


box = Box(3)
print(box.value)

cfg: Config = {"name": "demo", "count": 2}
print(sorted(cfg.items()))

fn: Callable[[Any], Any] = lambda x: x
print(fn(accepts("a")))
