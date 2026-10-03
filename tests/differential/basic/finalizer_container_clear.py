"""Purpose: a finalizer-bearing object held in a container is released (and its
``__del__`` runs) when the container drops the reference via ``clear()``/removal.

STATUS: native differential pass. Container `clear()`, `pop()`, and module-global
container `del` release the contained finalizer-bearing object at the
CPython-visible boundary.
"""

events = []


class A:
    def __init__(self, tag: int) -> None:
        self.tag = tag

    def __del__(self) -> None:
        events.append(self.tag)


def run() -> None:
    bag = []
    bag.append(A(1))
    bag.append(A(2))
    # clear() drops both references -> both finalizers run here, before the print.
    bag.clear()
    print("after clear", sorted(events))


run()

# pop() also releases the popped element.
events.clear()
bag2 = [A(10), A(11)]
bag2.pop()  # releases A(11)
print("after pop", sorted(events))
del bag2  # releases A(10)
print("after del bag2", sorted(events))
print("done")


class ReentrantList(list):
    __slots__ = ("label",)

    def __init__(self):
        super().__init__()
        self.label = "live"


class ListReentry:
    def __init__(self, owner):
        self.owner = owner

    def __del__(self):
        events.append((list.__len__(self.owner), self.owner.label))
        list.append(self.owner, "reentered")


events.clear()
reentrant = ReentrantList()
list.append(reentrant, ListReentry(reentrant))
list.clear(reentrant)
print("list subclass clear reentry", events, reentrant, reentrant.label)

import gc
import weakref


class CyclicList(list):
    __slots__ = ("slot", "__dict__", "__weakref__")


def make_list_cycle():
    value = CyclicList()
    value.slot = value
    value.dynamic = value
    list.append(value, value)
    return weakref.ref(value)


cycle = make_list_cycle()
gc.collect()
print("list subclass cycle collected", cycle() is None)


class FinalListA(list):
    __slots__ = ("label",)

    def __del__(self):
        events.append(("old class", self.label, list.__len__(self)))


class FinalListB(list):
    __slots__ = ("label",)

    def __del__(self):
        events.append(("new class", self.label, list.__len__(self)))


def release_reassigned_list():
    value = FinalListA([1, 2])
    value.label = "retained"
    value.__class__ = FinalListB


events.clear()
release_reassigned_list()
gc.collect()
print("list reassigned finalizer", events)


class FailedList(list):
    def __init__(self):
        super().__init__([1])
        self.label = "partial"
        raise ValueError("constructor failed")

    def __del__(self):
        events.append((self.label, list.__len__(self)))


events.clear()
try:
    FailedList()
except ValueError:
    pass
gc.collect()
print("list failed constructor finalizer", events)
