"""A coroutine reload keeps exact values without narrowing subclass values."""


class Tick:
    def __await__(self):
        yield None


class IntLike(int):
    def __add__(self, other):
        return f"overridden:{other}"


async def exact():
    return (2**100 + (2 if await Tick() else 3)) >> (1 if await Tick() else 2)


async def mixed(use_override):
    return (IntLike(8) if use_override else 8) + (2 if await Tick() else 3)


def complete(coroutine):
    try:
        while True:
            coroutine.send(None)
    except StopIteration as completed:
        return completed.value


print(complete(exact()))
print(complete(mixed(False)))
print(complete(mixed(True)))
