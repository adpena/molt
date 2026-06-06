# MOLT_META: xfail=molt xfail_reason=task60_init_full_binding_exc_swallow
"""Purpose: an exception raised inside a constructor `__init__` that requires
FULL ARGUMENT BINDING (a variadic `*args`/`**kwargs` parameter, or a
keyword-only parameter) must propagate out of the `ClassName(...)` construct
expression — exactly as CPython does.

Regression for task #60 (P0 silent control-flow elision / silent exception
swallow, native). Root cause (runtime): `call_type_with_builder`'s
`InitArgPolicy::ForwardArgs` arm (runtime/molt-runtime/src/call/bind.rs:1454)
invokes a full-binding `__init__` via `molt_call_bind` but — UNLIKE every other
`molt_call_bind` call site in that file (lines 100-104, 792-795, 835-837,
1383-1385, 2822-2826, ...) — does NOT check `exception_pending` afterward. It
unconditionally returns the (partially-constructed) `inst_bits`. Because the
returned value is a real instance (not `none`), the IC dispatch propagation
guards (`if res.is_none() && exception_pending` at bind.rs 2091/2131/2205) do
not fire, the construct-site `check_exception` is satisfied by the live
instance, and the raise is silently dropped: the `try` sees no exception, or a
plain construct continues to the next statement.

The fast path for SIMPLE (non-full-binding) `__init__` (bind.rs 2689-2826) and
plain methods / `__new__` already check the pending flag, so this is specific to
full-binding `__init__`:

  * `def __init__(self, ea, **kw)`     -> SKIPPED  (this file's `K`/`R`/`S` cells)
  * `def __init__(self, *args)`        -> SKIPPED  (`A`)
  * `def __init__(self, *, ea)`        -> SKIPPED  (`O` keyword-only)
  * `def __init__(self, ea)`           -> fires  (simple, fast path)
  * `def __init__(self, a,b,c,d,e,f)`  -> fires  (6 positional, no full binding)
  * regular method with `**kw`         -> fires  (method-call path checks)
  * `def __new__(cls, *a, **kw)`       -> fires  (`__new__` path checks)

Must be byte-identical to CPython 3.12 / 3.13 / 3.14 on every target.
"""


# ── K: **kwargs __init__, conditional raise (the csv DictWriter shape). ──
class K:
    def __init__(self, ea, **kw):
        if ea == "bad":
            raise ValueError("K: " + ea)
        self.ea = ea


try:
    K("bad")
    print("K: NO RAISE (bug)")
except ValueError as exc:
    print("K caught:", exc)


# ── R: **kwargs __init__, UNCONDITIONAL raise (no branch at all). ──
class R:
    def __init__(self, ea, **kw):
        raise ValueError("R: always")


try:
    R("x")
    print("R: NO RAISE (bug)")
except ValueError as exc:
    print("R caught:", exc)


# ── A: *args __init__, unconditional raise. ──
class A:
    def __init__(self, *args):
        raise ValueError("A: args")


try:
    A(1, 2, 3)
    print("A: NO RAISE (bug)")
except ValueError as exc:
    print("A caught:", exc)


# ── O: keyword-only __init__ (also requires full binding), raise. ──
class O:
    def __init__(self, *, ea):
        raise ValueError("O: kwonly")


try:
    O(ea="x")
    print("O: NO RAISE (bug)")
except ValueError as exc:
    print("O caught:", exc)


# ── S: the exact csv membership shape — attr = x.lower(); not-in-set; raise;
#       then a **fmtparams-splat call after the raise (the original report). ──
def _mk(csvfile, dialect, **fmtparams):
    return (csvfile, dialect, len(fmtparams))


class S:
    def __init__(
        self, csvfile, fieldnames, extrasaction="raise", dialect="excel", **fmtparams
    ):
        self.fieldnames = fieldnames
        self.extrasaction = extrasaction.lower()
        if self.extrasaction not in {"raise", "ignore"}:
            raise ValueError(
                "extrasaction (%s) must be 'raise' or 'ignore'" % extrasaction
            )
        self._writer = _mk(csvfile, dialect=dialect, **fmtparams)


try:
    S(None, ["a"], extrasaction="bad")
    print("S: NO RAISE (bug)")
except ValueError as exc:
    print("S caught:", exc)
print("S ok:", S(None, ["a"], extrasaction="RAISE").extrasaction)


# ── Bare construct (no try): the raise must reach the top level. ──
class B:
    def __init__(self, **kw):
        if kw.get("boom"):
            raise ValueError("B: boom")
        self.ok = True


print("B ok:", B(boom=False).ok)


# ── Controls that MUST already pass (simple __init__ / method / __new__). ──
class Simple:
    def __init__(self, ea):
        if ea == "bad":
            raise ValueError("Simple: " + ea)


try:
    Simple("bad")
    print("Simple: NO RAISE")
except ValueError as exc:
    print("Simple caught:", exc)


class SixArg:
    def __init__(self, a, b, c, d, e, f):
        raise ValueError("SixArg")


try:
    SixArg(1, 2, 3, 4, 5, 6)
    print("SixArg: NO RAISE")
except ValueError as exc:
    print("SixArg caught:", exc)


class Meth:
    def __init__(self):
        pass

    def m(self, x, **kw):
        raise ValueError("Meth.m")


try:
    Meth().m(1)
    print("Meth: NO RAISE")
except ValueError as exc:
    print("Meth caught:", exc)


class NewRaise:
    def __new__(cls, *a, **kw):
        raise ValueError("NewRaise")


try:
    NewRaise(1)
    print("NewRaise: NO RAISE")
except ValueError as exc:
    print("NewRaise caught:", exc)

print("end")
