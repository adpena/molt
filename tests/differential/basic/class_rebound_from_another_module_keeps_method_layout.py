"""Purpose: methods and annotated functions keep their class's field layout after another module rebinds the class name to a same-shaped class (HF-220)."""

import class_identity_support.rebind as rebind
import class_identity_support.shapes as shapes
from class_identity_support.shapes import P


def read_imported(p: P):
    return p.a


original = shapes.P
print(shapes.read_a(original()), read_imported(original()))
rebind.swap()
print(shapes.P is shapes.Q, P is original)
q = shapes.Q()
print(original.get_a(q))
original.set_b(q, 99)
print(q.a, q.b)
print(shapes.read_a(q), read_imported(q))
print(original.get_a(original()), shapes.read_a(original()), read_imported(original()))
