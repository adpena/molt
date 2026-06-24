"""Purpose: doc 50 m86_dc — a ``@dataclass`` instance (non-folded allocation
path, no ``__del__`` of its own) holding an object-valued field must itself be
RELEASED when its binding function returns, cascading the field release (#86's
inline-field authority) so the child's ``__del__`` fires.

CPython prints ``LEAF del`` before ``end``; molt now matches it byte-for-byte
with a clean exit under ``MOLT_ASSERT_NO_LEAK``.

STATUS (2026-06-24): PASSES — a regression guard, no longer a known-bad anchor.
The historical gap: the owned ``DATACLASS_NEW_VALUES`` result bound to a
never-read local (``n = Node(Leaf())``) was absent from the last-use drop scan,
so the parent instance was never released and the field-finalizer cascade never
started (``dealloc_object=0`` — a drop-placement gap on the dataclass owner).
Fixed by the round-12 native finalizer-drop arc merged via ``df8f080d0``
(fin58-recovery): ``fe951364d`` adds the §1b dead-result scan that DecRefs an
owned, zero-use result at its defining op, and ``08a8cf5a0`` keeps a
``__del__``-bearing instance (the ``Leaf``) heap-allocated with a live refcount
so the release reaches the finalizer. Distinct from #58 (ordering of placed
drops) and #86 (field release once the parent IS freed — closed, ac73ab954).
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
