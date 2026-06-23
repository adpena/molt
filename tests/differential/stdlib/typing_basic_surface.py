"""Purpose: differential coverage for typing basics used by Click/Trio/tinygrad."""

from typing import (
    Any,
    Callable,
    Generator,
    Generic,
    Literal,
    Sequence,
    Type,
    TypedDict,
    TypeVar,
    get_args,
    get_origin,
)

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

print(isinstance([], Sequence))
print(get_origin(Sequence[int]).__name__, get_args(Generator[int, None, None])[0])
print(get_origin(Type[int]).__name__)
