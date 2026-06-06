"""Regression for task #50 (cross-chunk dangling class-reference).

CPython runs this cleanly (prints b0..b7 then OK). molt used to SIGSEGV on native
(and raise a spurious `TypeError: object.__new__ expects type` on chunk-split paths
that take a non-direct store) because the module body (`molt_main`) is split into
multiple `molt_module_chunk_N` functions once it exceeds the native chunk-op
threshold (default 1400, src/molt/cli.py). A class defined in one chunk and
instantiated in a later chunk had its `class_value_name` SSA reference dangle across
the chunk boundary; the constructor-fold fast path in
src/molt/frontend/visitors/calls.py reused that stale SSA name without a liveness
check, and lowering degraded it to a `CONST_STR` of the variable name (e.g. literal
"v312"). The runtime then saw a string where a type was expected.

Eight small generic classes are enough to force a >=2-chunk split at the default
threshold, with the class definitions in an earlier chunk than the instantiations.
This is NOT specific to PEP 695 generics — plain classes hit the same bug across a
chunk boundary; generics merely cross the op-cost threshold sooner.

Fixed: route the molt_main constructor-fold branch through
`_current_module_static_class_ref` (which performs the `self.globals` liveness
guard) and fall back to MODULE_GET_ATTR when the class SSA value is not live in the
current chunk. Full diagnosis + fix notes: tmp/baton_task50_generic_class_sigsegv.md.
Passes byte-identical to CPython 3.12/3.13/3.14.
"""


class Box0[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


class Box1[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


class Box2[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


class Box3[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


class Box4[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


class Box5[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


class Box6[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


class Box7[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


if __name__ == "__main__":
    b0 = Box0(0)
    print("b0", b0.get(), b0.replace(0 + 1))
    b1 = Box1(1)
    print("b1", b1.get(), b1.replace(1 + 1))
    b2 = Box2(2)
    print("b2", b2.get(), b2.replace(2 + 1))
    b3 = Box3(3)
    print("b3", b3.get(), b3.replace(3 + 1))
    b4 = Box4(4)
    print("b4", b4.get(), b4.replace(4 + 1))
    b5 = Box5(5)
    print("b5", b5.get(), b5.replace(5 + 1))
    b6 = Box6(6)
    print("b6", b6.get(), b6.replace(6 + 1))
    b7 = Box7(7)
    print("b7", b7.get(), b7.replace(7 + 1))
    print("OK")
