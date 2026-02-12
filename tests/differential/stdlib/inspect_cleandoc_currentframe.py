"""Purpose: differential coverage for inspect.cleandoc/currentframe helpers."""

import inspect


def sample():
    """
    line1
        line2
    """
    frame = inspect.currentframe()
    return frame.f_code.co_name, frame.f_lineno


def gen():
    yield 1


name, line = sample()
print(name, isinstance(line, int))
print(inspect.cleandoc(sample.__doc__))
print(inspect.isgeneratorfunction(gen))
