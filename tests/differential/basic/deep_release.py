"""Terminal ownership must not use stack proportional to graph depth (HF-660).

Markers surround each final reference release; a process crash, skipped release,
or a retained alias cannot masquerade as a successful differential execution.
Sets use identity-hashed nodes because mutable sets cannot contain sets.
"""

from collections import deque


class Node:
    def __init__(self, next):
        self.next = next


class SlotNode:
    __slots__ = ("next",)

    def __init__(self, next):
        self.next = next


class Witness(Exception):
    def __init__(self, name):
        self.name = name

    def __del__(self):
        print("released", self.name)


def cell(value):
    def capture():
        return value

    return capture.__closure__[0]


def generator(value):
    yield value


def exception(value):
    result = Exception()
    result.__cause__ = value
    return result


def method(value):
    return Node(value).__init__


def release(name, wrap):
    value = Witness(name)
    for _ in range(100_000):
        value = wrap(value)
    print("before", name)
    del value
    print("after", name)


release("list", lambda value: [value])
release("tuple", lambda value: (value,))
release("dict", lambda value: {"next": value})
release("set", lambda value: {Node(value)})
release("frozenset", lambda value: frozenset((value,)))
release("deque-list", lambda value: deque(([value],)))
release("instance", Node)
release("slots", SlotNode)
# CPython 3.12 does not put deques, cells or generators in its trashcan. A list at
# each link makes their reference oracle stack-safe while still traversing
# their real terminal payloads in both runtimes.
release("cell-list", lambda value: cell([value]))
release("generator-list", lambda value: generator([value]))
release("bound-method", method)
release("exception-cause", exception)

# No last loop local may retain a second reference to this root. Its only owner
# after module completion is the module namespace, retired during shutdown.
deep_global = Witness("global teardown")
for _ in range(100_000):
    deep_global = [deep_global]
print("deep global ready for teardown")
