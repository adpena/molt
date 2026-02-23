"""Purpose: differential coverage for kwonly params returned from class methods.

Exercises the call_bind path where kwonly argument values are heap-allocated
(strings) and must survive the CallArgs cleanup.  Regression test for a
use-after-free where callargs_dec_ref_all freed the kwonly value before the
caller could use the return value.
"""

_UNSET = object()


class Checker:
    def check(self, key: str) -> bool:
        return False

    def get(self, key, *, fallback=_UNSET):
        result = self.check(str(key))
        if not result:
            if fallback is not _UNSET:
                return fallback
        return "default"


c = Checker()

# No kwonly — default returned
r1 = c.get("k")
print("no_fallback:", r1)

# String kwonly — must survive callargs cleanup
r2 = c.get("k", fallback="HELLO")
print("str_fallback:", r2)
print("str_match:", r2 == "HELLO")

# Integer kwonly — inline NaN-boxed, no heap
r3 = c.get("k", fallback=42)
print("int_fallback:", r3)
print("int_match:", r3 == 42)

# None kwonly
r4 = c.get("k", fallback=None)
print("none_fallback:", r4)
print("none_match:", r4 is None)

# Tuple kwonly — heap-allocated
r5 = c.get("k", fallback=(1, 2, 3))
print("tuple_fallback:", r5)
print("tuple_match:", r5 == (1, 2, 3))


# Multiple kwonly params, only some returned
class Multi:
    def do_thing(self, key: str) -> bool:
        return False

    def fetch(self, key, *, raw=False, fallback=_UNSET, mode="default"):
        exists = self.do_thing(str(key))
        if not exists:
            if fallback is not _UNSET:
                return fallback
        if raw:
            return "raw:" + str(mode)
        return "cooked:" + str(mode)


m = Multi()
r6 = m.fetch("k", fallback="FB_VAL")
print("multi_fallback:", r6)
print("multi_match:", r6 == "FB_VAL")

r7 = m.fetch("k", raw=True, mode="fast")
print("multi_raw:", r7)

r8 = m.fetch("k", fallback="OTHER", raw=False, mode="slow")
print("multi_fb2:", r8)
print("multi_fb2_match:", r8 == "OTHER")


# Inheritance: kwonly params through super() dispatch
class Base:
    def has(self, k: str) -> bool:
        return False

    def get(self, k, *, fallback=_UNSET):
        if not self.has(str(k)):
            if fallback is not _UNSET:
                return fallback
        return "base_default"


class Child(Base):
    def has(self, k: str) -> bool:
        return k == "found"


ch = Child()
r9 = ch.get("missing", fallback="child_fb")
print("child_fallback:", r9)
print("child_match:", r9 == "child_fb")

r10 = ch.get("found", fallback="should_not_use")
print("child_found:", r10)
print("child_found_match:", r10 == "base_default")
