from molt.stdlib import copy, fnmatch, inspect, pprint, string, sys, traceback, typing


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
