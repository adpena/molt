# MOLT_META: expect_fail=molt expect_fail_reason=dataclass_instance_drop_never_placed_so_child_finalizer_skipped_doc50_m86dc
"""Purpose: doc 50 m86_dc — a ``@dataclass`` instance (non-folded allocation
path, no ``__del__`` of its own) holding an object-valued field must itself be
RELEASED when its binding function returns, cascading the field release (#86's
inline-field authority) so the child's ``__del__`` fires.

CPython prints ``LEAF del`` before ``end``; molt never frees the dataclass
instance (``dealloc_object=0`` — a drop-PLACEMENT gap on the DATACLASS_NEW
result, layer 1 of doc 50), so the cascade never starts. Module-free isolation
repro promoted to a durable known-bad anchor — a DEBT WITH AN OWNER (doc 50
slice 4), not an accepted state. Distinct from #58 (ordering of placed drops)
and #86 (field release once the parent IS freed — closed, ac73ab954).
"""

from dataclasses import dataclass


class Leaf:
    def __del__(self) -> None:
        print("LEAF del")


@dataclass
class Node:
    child: object


def mk() -> None:
    n = Node(Leaf())


mk()
print("end")
