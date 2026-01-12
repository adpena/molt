from molt.stdlib import (
    copy,
    fnmatch,
    functools,
    inspect,
    itertools,
    operator,
    pprint,
    string,
    sys,
    traceback,
    typing,
)


def foo(a, b=1):
    return a + b


print(copy.copy([1, 2]) == [1, 2])
print(copy.deepcopy({"a": [1, 2]}) == {"a": [1, 2]})
print(fnmatch.fnmatch("hello.txt", "*.txt"))
print(string.capwords("hello world"))
print(pprint.pformat({"b": 2, "a": 1}))
print(repr(typing.TypeVar("T")))
print(inspect.signature(foo))
try:
    1 / 0
except Exception as exc:
    print(traceback.format_exception_only(type(exc), exc)[0].strip())
print(sys.getrecursionlimit() > 0)


def add(a, b):
    return a + b


part = functools.partial(add, 2)
print(part(3))


def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


fib = functools.lru_cache(maxsize=2)(fib)


print(fib(5))
print(fib.cache_info().hits >= 0)
print(functools.reduce(add, [1, 2, 3], 0))

print(list(itertools.chain([1, 2], [3])))
print(list(itertools.islice([0, 1, 2, 3, 4], 1, 5, 2)))
print(list(itertools.islice(iterable=[0, 1, 2, 3, 4], start=1, stop=4, step=2)))
print(list(itertools.repeat("x", 3)))


class Box:
    def __init__(self, value):
        self.value = value


print(operator.add(1, 2))
print(operator.mul(3, 4))
print(operator.eq(5, 5))
print(operator.itemgetter(1, 0)([10, 20]))
print(operator.attrgetter("value")(Box(7)))
print(operator.methodcaller("get", "a")({"a": 9}))
