"""Generator and coroutine state lives in closure-payload slots. Stores must keep
exactly one reference per slot across first heap stores, immediate/heap
transitions and self-assignment, and loads must hand out live values."""

import gc
import weakref


class Token:
    def __init__(self, name):
        self.name = name


def churn(n):
    held = 0  # an immediate first
    for i in range(n):
        if i % 3 == 0:
            held = Token(f"t{i}")  # immediate -> heap, heap -> heap
        elif i % 3 == 1:
            held = held  # self-assignment
        else:
            held = i  # heap -> immediate
        yield held.name if isinstance(held, Token) else held


def survivor():
    token = Token("kept")
    ref = weakref.ref(token)
    yield ref() is token
    del token
    gc.collect()
    yield ref() is None


def counter():
    total = 0

    def add(value):
        nonlocal total
        total = total + value
        return total

    return add


print(list(churn(10)))
print(list(survivor()))
add = counter()
print([add(Token("x").name.__len__()) for _ in range(4)])
