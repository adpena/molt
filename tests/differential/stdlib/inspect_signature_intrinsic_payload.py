"""Purpose: exercise inspect.signature lowering payload for full arg shapes."""

import inspect


def f(a, b=2, /, c=3, *args, d, e=5, **kwargs):
    return (a, b, c, args, d, e, kwargs)


def g(x, /, y, *, z):
    return (x, y, z)


print(str(inspect.signature(f)))
print(str(inspect.signature(g)))



def actual_receiver(self: int, a, /, b=2, *args: str, size=4, **options) -> bool:
    return True


class Receiver:
    call = actual_receiver


def wrapper(*args, **kwargs):
    pass


wrapper.__wrapped__ = actual_receiver
for callable_value in (actual_receiver, Receiver().call, wrapper):
    spec = inspect.getfullargspec(callable_value)
    print(type(spec) is inspect.FullArgSpec, spec)
