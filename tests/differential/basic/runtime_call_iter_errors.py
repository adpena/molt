"""PyObject_GetIter errors must survive every runtime-call realization."""

for value in (1, 1.0, True, None):
    try:
        iter(value)
    except TypeError as error:
        print(type(value).__name__, str(error))
    else:
        raise AssertionError("a non-iterable must raise TypeError")


class BrokenIterable:
    def __iter__(self):
        raise ValueError("iteration callback")


try:
    iter(BrokenIterable())
except ValueError as error:
    print(type(error).__name__, str(error))
else:
    raise AssertionError("iteration callback exceptions must propagate")
