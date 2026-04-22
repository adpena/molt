# Parity test: iterators and iteration protocols
# All output via print() for diff comparison

print("=== iter() and next() basics ===")
it = iter([10, 20, 30])
print(next(it))
print(next(it))
print(next(it))
try:
    next(it)
except StopIteration:
    print("StopIteration raised")

print("=== next() with default ===")
it = iter([1])
print(next(it, "default"))
print(next(it, "default"))
print(next(it, None))

print("=== iter() on string ===")
it = iter("abc")
print(next(it))
print(next(it))
print(next(it))

print("=== iter() on dict ===")
d = {"a": 1, "b": 2, "c": 3}
keys = list(iter(d))
print(sorted(keys))

print("=== Custom __iter__/__next__ ===")


class Countdown:
    def __init__(self, start):
        self.start = start

    def __iter__(self):
        self.current = self.start
        return self

    def __next__(self):
        if self.current <= 0:
            raise StopIteration
        val = self.current
        self.current -= 1
        return val


print(list(Countdown(5)))
print(list(Countdown(0)))

print("=== Iterable vs iterator ===")


class Squares:
    def __init__(self, n):
        self.n = n

    def __iter__(self):
        for i in range(self.n):
            yield i * i


sq = Squares(5)
print(list(sq))
print(list(sq))  # should work again since __iter__ returns new generator

print("=== zip basics ===")
print(list(zip([1, 2, 3], ["a", "b", "c"])))
print(list(zip([1, 2], [10, 20, 30])))
print(list(zip([], [1, 2])))
print(list(zip([1, 2, 3], "abc", [True, False, None])))

print("=== zip strict ===")
try:
    list(zip([1, 2], [10, 20, 30], strict=True))
except ValueError:
    print("zip strict error: ValueError")

print(list(zip([1, 2], [10, 20], strict=True)))

print("=== enumerate ===")
print(list(enumerate(["a", "b", "c"])))
print(list(enumerate(["x", "y"], start=5)))
print(list(enumerate([])))

print("=== reversed basics ===")
print(list(reversed([1, 2, 3, 4])))
print(list(reversed("hello")))
print(list(reversed(range(5))))
print(list(reversed((10, 20, 30))))

print("=== reversed on custom type ===")


class ReversibleContainer:
    def __init__(self, data):
        self.data = data

    def __reversed__(self):
        return iter(self.data[::-1])

    def __iter__(self):
        return iter(self.data)


rc = ReversibleContainer([1, 2, 3, 4, 5])
print(list(reversed(rc)))
print(list(rc))

print("=== map/filter ===")
print(list(map(str, [1, 2, 3])))
print(list(map(lambda x: x * 2, [1, 2, 3])))
print(list(filter(None, [0, 1, "", "a", [], [1]])))
print(list(filter(lambda x: x > 2, [1, 2, 3, 4, 5])))

print("=== map with multiple iterables ===")
print(list(map(lambda a, b: a + b, [1, 2, 3], [10, 20, 30])))

print("=== itertools-like patterns with generators ===")


def take(n, iterable):
    it = iter(iterable)
    for _ in range(n):
        try:
            yield next(it)
        except StopIteration:
            return


def repeat_val(val, times):
    for _ in range(times):
        yield val


def chain(*iterables):
    for it in iterables:
        yield from it


print(list(take(3, range(100))))
print(list(take(5, [1, 2])))
print(list(repeat_val("x", 4)))
print(list(chain([1, 2], [3, 4], [5])))

print("=== Generator send ===")


def accumulator():
    total = 0
    while True:
        val = yield total
        if val is None:
            break
        total += val


gen = accumulator()
print(next(gen))
print(gen.send(10))
print(gen.send(20))
print(gen.send(5))

print("=== any() / all() with iterators ===")
print(any(x > 3 for x in [1, 2, 3, 4, 5]))
print(any(x > 10 for x in [1, 2, 3]))
print(all(x > 0 for x in [1, 2, 3]))
print(all(x > 0 for x in [1, 0, 3]))
print(any([]))
print(all([]))

print("=== sorted with key ===")
print(sorted([3, 1, 4, 1, 5], reverse=True))
print(sorted(["banana", "apple", "cherry"], key=len))
print(sorted([(1, "b"), (2, "a"), (1, "a")], key=lambda x: (x[0], x[1])))

print("=== sum / min / max on iterators ===")
print(sum(x * x for x in range(5)))
print(min(x for x in [3, 1, 4, 1, 5]))
print(max(x for x in [3, 1, 4, 1, 5]))
print(sum(range(10)))
