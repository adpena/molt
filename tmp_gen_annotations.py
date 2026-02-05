from typing import Iterable


def gen(x: int) -> Iterable[int]:
    if x > 0:
        yield x


print(list(gen(1)))
