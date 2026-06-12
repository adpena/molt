# MOLT_META: xfail=molt xfail_reason=finalizer-container-release-boundary
"""Purpose: a finalizer-bearing object held in a container is released (and its
``__del__`` runs) when the container drops the reference via ``clear()``/removal.

STATUS: expected-fail. The current native path drops the container contents
without running the element finalizers at `clear()`/`pop()`/container `del`, so
the observed event list stays empty. Keep the CPython-strict assertions and let
the harness flip this to XPASS-failure when container-owned releases dispatch
`__del__` at the boundary.
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
