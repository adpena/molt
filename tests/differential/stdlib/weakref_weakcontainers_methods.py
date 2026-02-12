"""Purpose: differential coverage for lowered weakref container methods."""

import gc
import weakref


class Value:
    pass


value = Value()
values = weakref.WeakValueDictionary()
values["a"] = value
print("wvd-len", len(values))
print("wvd-get", values["a"] is value)
print("wvd-refs", len(values.valuerefs()))
print("wvd-iterrefs", len(list(values.itervaluerefs())))
print("wvd-pop", values.pop("a") is value)
print("wvd-len", len(values))
values["a"] = value
del value
gc.collect()
print("wvd-len-gc", len(values))


class Node:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        return hash(self.n)

    def __eq__(self, other):
        return isinstance(other, Node) and self.n == other.n


n1 = Node(1)
n2 = Node(1)
weak_set = weakref.WeakSet()
weak_set.add(n1)
weak_set.add(n2)
print("ws-len", len(weak_set))
print("ws-contains", n1 in weak_set, n2 in weak_set)
print("ws-pop", isinstance(weak_set.pop(), Node))
print("ws-len2", len(weak_set))
weak_set.add(n1)
weak_set.discard(n2)
print("ws-len3", len(weak_set))
