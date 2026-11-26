import sys


def chain(*iterables):
    for iterable in iterables:
        for item in iterable:
            yield item


class Observer:
    def __iter__(self):
        return self

    def __next__(self):
        caller = sys._getframe(1)
        print("observer", caller.f_code.co_name)
        raise ValueError("inner step")


def consume():
    for item in chain(range(2), Observer()):
        print("item", item)


try:
    consume()
except ValueError as exc:
    print("caught", str(exc))
