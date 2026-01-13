def outer():
    def inner():
        return 1

    return inner


print(outer().__qualname__)


class C:
    def method(self):
        def inner():
            return 2

        return inner


print(C().method().__qualname__)

f = lambda x: (lambda y: x + y)  # noqa: E731
print(f.__qualname__)
print(f(1).__qualname__)
