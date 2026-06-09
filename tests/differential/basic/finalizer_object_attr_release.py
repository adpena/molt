"""Purpose: a finalizer-bearing (non-folded) parent must RELEASE its object-valued
inline attribute fields when it is freed, so the child objects' ``__del__`` runs.

Regression for #86 (Design A — single field-ownership authority). A class that
defines ``__del__`` declines molt's constructor-fold (to preserve finalizer
timing), so its instances reach the runtime free path. Before the fix, the runtime
free released only the trailing ``__dict__`` + class — never the inline typed
attribute fields — so every object-valued attribute of such an instance LEAKED and
its ``__del__`` was silently skipped. The fix makes the runtime free the single
owner that releases inline fields (gated on ``HEADER_FLAG_HAS_PTRS``; folded
objects release via the compiler and are stack-promoted, so no double-free).

Drops go through the function-return path (a function-local owned object released
at return), which avoids the unrelated #63 (loop-body ``del``) and the dataclass /
container-clear DecRef-PLACEMENT gaps. Order and counts are pinned to CPython.
"""

events = []


class Leaf:
    def __init__(self, tag: int) -> None:
        self.tag = tag

    def __del__(self) -> None:
        events.append(("leaf", self.tag))


class Node:
    def __init__(self, tag: int) -> None:
        # Two object-valued fields + a primitive field: the primitive must NOT be
        # mis-released, both objects MUST be released.
        self.left = Leaf(tag)
        self.right = Leaf(tag + 100)
        self.n = tag

    def __del__(self) -> None:
        events.append(("node", self.n))


def drop_one(tag: int) -> None:
    Node(tag)  # owned, released at function return -> node + both leaves finalize


# 1. Single finalizer-bearing parent: parent + both children finalize.
drop_one(1)
print("after-one", sorted(events))
events.clear()

# 2. A short run so the release path is exercised repeatedly (each iteration frees
#    one Node and its two Leaf children — 3 finalizers per iteration).
for i in range(5):
    drop_one(i)
print("count", len(events))  # 5 nodes * 3 = 15

# 3. Nested object graph: a parent whose child also owns a finalizer-bearing child.
events.clear()


class Holder:
    def __init__(self, tag: int) -> None:
        self.inner = Node(tag)  # Node itself owns two Leaves

    def __del__(self) -> None:
        events.append(("holder", 0))


def drop_holder() -> None:
    Holder(7)


drop_holder()
# Holder + its Node + the Node's two Leaves all finalize.
print("nested", sorted(events))
print("done")
