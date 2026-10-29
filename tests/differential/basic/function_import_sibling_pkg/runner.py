"""Every import of `.sibling` here sits in a function body."""

import contextlib


def compute_twice(value):
    from .sibling import compute

    return compute(value) * 2


@contextlib.contextmanager
def sibling_scope(tag):
    from .sibling import scope

    with scope(tag) as value:
        yield value


async def sibling_coroutine(value):
    from .sibling import compute

    return compute(value)


def sibling_module_name():
    from . import sibling

    return sibling.__name__
