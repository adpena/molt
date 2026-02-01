"""Purpose: differential coverage for itertools laziness."""

import itertools

calls = []

def gen():
    calls.append("start")
    yield 1
    calls.append("yielded")
    yield 2

it = itertools.islice(gen(), 1)
print(calls)
print(list(it))
print(calls)
