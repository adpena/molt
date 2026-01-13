inc = lambda x: x + 1  # noqa: E731
print(inc(2))


def outer():
    x = 5
    return lambda y: x + y


print(outer()(7))


def default_capture():
    x = 3
    fn = lambda y=x: y  # noqa: E731
    x = 9
    return fn()


print(default_capture())


def varargs():
    fn = lambda *args, **kwargs: (args, kwargs)  # noqa: E731
    return fn(1, 2, a=3, b=4)


print(varargs())


def kwonly():
    fn = lambda *, x=1, y=2: x + y  # noqa: E731
    return fn(y=3)


print(kwonly())


def posonly():
    fn = lambda x, /, y: (x, y)  # noqa: E731
    return fn(1, 2)


print(posonly())

try:
    (lambda x, /, y: (x, y))(x=1, y=2)  # noqa: E731
except TypeError as exc:
    print(type(exc).__name__)


def nested_lambda():
    return (lambda x: (lambda y: x + y))(2)(3)


print(nested_lambda())
