"""Purpose: doc 50 m86_dc — a ``@dataclass`` instance (non-folded allocation
path, no ``__del__`` of its own) holding an object-valued field must itself be
RELEASED when its binding function returns, cascading the field release (#86's
inline-field authority) so the child's ``__del__`` fires.

CPython prints ``LEAF del`` before ``end``. Molt must place the release on the
fresh ``DATACLASS_NEW_VALUES`` owner after metadata attachment, not merely drop
the field operands; otherwise the parent instance survives and the field
finalizer cascade never starts. Distinct from #58 (ordering of placed drops) and
#86 (field release once the parent IS freed — closed, ac73ab954).
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
