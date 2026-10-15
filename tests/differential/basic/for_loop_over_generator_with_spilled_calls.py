"""Purpose: a for loop ends cleanly over a generator whose body makes spilled calls (HF-217).

Each loop step advances the generator through one unboxed iterator step that
returns its value through a scratch word. The generator body makes dynamic
calls with many arguments, which compiled code may pass through the same
scratch region, including the call right before the generator finishes and
the call in its exception path. The step must write its value word after that
code runs, so the loop sees each yielded value and the final exhausted step
releases nothing it does not own.
"""


def collect(items, a, b, c, d, e, f):
    return len(items) + a + b + c + d + e + f


callables = [collect]


def numbers(n):
    for i in range(n):
        yield callables[0]([i] * i, i, 1, 2, 3, 4, 5)
    callables[0](["done"], 0, 0, 0, 0, 0, 0)


def failing(n):
    for i in range(n):
        yield callables[0]([i], i, 0, 0, 0, 0, 0)
    callables[0](["fail"], 0, 0, 0, 0, 0, 0)
    raise ValueError("generator failed after its last value")


total = 0
for value in numbers(20):
    total += value
print(total)

seen = []
try:
    for value in failing(3):
        seen.append(value)
except ValueError as error:
    print(seen, error)

pairs = list(zip(numbers(4), numbers(5)))
print(pairs)
print(sum(numbers(50)), max(numbers(7)))
