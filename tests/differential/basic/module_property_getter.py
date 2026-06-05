# MOLT_META: expect_fail=molt expect_fail_reason=backend_phi_blockarg_miswire_inlined_property_fast_path
"""Purpose: a property getter on a module-level class accessed at module
scope must invoke the getter, not return None.

XFAIL (pre-existing BACKEND bug, not the frontend super fix in this change):
the FIRST guarded property access at module scope returns None. Root cause is in
the backend SimpleIR PHI -> block-argument lowering: the guarded property fast
path (`IF guard: <inlined getter> ELSE: get_attr_generic_ptr; PHI`) computes the
inlined getter result into the fast-path block-arg slot, but the merge block
stores the SLOW-path value into the result slot for the fast predecessor too, so
the merged value is the slow-path (None on the cold first access). Reproduces
only at module scope (function scope is correct) and independently of the super
fold. Fix lives in runtime/molt-backend (tir/lower_to_simple.rs PHI edge wiring),
outside this change's frontend lane. See the session baton.
"""


class Temperature:
    def __init__(self, celsius: float) -> None:
        self._celsius = celsius

    @property
    def fahrenheit(self) -> float:
        return self._celsius * 9.0 / 5.0 + 32.0

    @property
    def label(self) -> str:
        return "T=" + str(self._celsius)


# Module-scope construction + property access (the bug site).
t = Temperature(100.0)
print(t.fahrenheit)
print(t.label)

t2 = Temperature(0.0)
print(t2.fahrenheit)
print(t2.label)


# Property with a setter, exercised at module scope.
class Box:
    def __init__(self) -> None:
        self._v = 1

    @property
    def v(self) -> int:
        return self._v * 10

    @v.setter
    def v(self, value: int) -> None:
        self._v = value


b = Box()
print(b.v)
b.v = 5
print(b.v)
