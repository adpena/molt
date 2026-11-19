"""Purpose: CPython parity for finalizer order in releases deeper than Molt's release bound.

CPython releases a dying graph recursively, each child with everything it
releases in turn, before the parent's next child (`_Py_Dealloc` defers only at
the C stack margin). Molt bounds its release depth; past the bound the order
of `__del__` calls and weakref callbacks must stay the same.
"""

import hashlib
import weakref


events = []
refs = []


class Node:
    def __init__(self, name, left=None, right=None):
        self.name = name
        self.left = left
        self.right = right

    def __del__(self):
        events.append(self.name)


def tree(name, depth):
    """A binary tree whose edges alternate list, tuple, dict and fields."""
    if depth == 0:
        node = Node(name)
    elif depth % 4 == 0:
        node = Node(name, [tree(name + "a", depth - 1), tree(name + "b", depth - 1)])
    elif depth % 4 == 1:
        node = Node(name, (tree(name + "a", depth - 1), tree(name + "b", depth - 1)))
    elif depth % 4 == 2:
        node = Node(name, {"a": tree(name + "a", depth - 1), "b": tree(name + "b", depth - 1)})
    else:
        node = Node(name, tree(name + "a", depth - 1), tree(name + "b", depth - 1))
    refs.append(weakref.ref(node, lambda ref, name=name: events.append("weak " + name)))
    return node


def report(label):
    digest = hashlib.sha256("\n".join(events).encode()).hexdigest()[:16]
    print(label, len(events), digest)
    print(label, "head", events[:12])
    print(label, "tail", events[-12:])
    print(label, "cleared", all(ref() is None for ref in refs))
    events.clear()
    refs.clear()


# Twelve levels of nodes cross the bound several times over.
root = tree("r", 12)
del root
report("tree")


# A deep list whose bottom item finalizes, beside a finalizing item in the
# outer list: list_dealloc releases items last to first.
bottom = Node("bottom")
nested = bottom
del bottom
for _ in range(100):
    nested = [nested]
outer = [Node("first"), nested, Node("last")]
del nested
del outer
report("list")
