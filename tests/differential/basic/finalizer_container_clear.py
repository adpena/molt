"""Purpose: a finalizer-bearing object held in a container is released (and its
``__del__`` runs) when the container drops the reference via ``clear()``/removal.

This pins the WORKING boundary of the Finalizer Lifetime Closure vertical (doc 50):
single-element container release is correct. The loop-append form (#63) and the
scope-exit timing form (#58) are tracked separately as open placement/ordering
slices; this file is the green anchor that must not regress as those land.
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
