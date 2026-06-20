"""Deque handle storage must own heap elements until transfer or release."""

from collections import deque


class Node:
    def __init__(self, name):
        self.name = name

    def __eq__(self, other):
        return isinstance(other, Node) and self.name == other.name


def make_stack_entry():
    root = Node("root")
    return (root, 0, root)


stack = deque([make_stack_entry()])
n, stage, new_n = stack.pop()
print("pop_tuple", stage, n.name, n is new_n)

d = deque([Node("getitem")])
borrowed = d[0]
d.clear()
print("getitem_survives_clear", borrowed.name)

d = deque(maxlen=1)
d.append(Node("evicted"))
d.append(Node("kept"))
print("bounded", d.pop().name)

d = deque([Node("copy")])
clone = d.copy()
d.clear()
print("copy_survives_clear", clone.pop().name)

d = deque([Node("old"), Node("slot")])
old = d[1]
d[1] = Node("new")
print("setitem_old_survives", old.name, d.pop().name)

d = deque([Node("deleted"), Node("tail")])
deleted = d[0]
del d[0]
print("delitem_old_survives", deleted.name, d.pop().name)

target = Node("remove")
d = deque([target, Node("remain")])
d.remove(target)
print("remove", d.pop().name)
