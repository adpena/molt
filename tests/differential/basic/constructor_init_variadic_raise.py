"""Purpose: an exception raised inside a constructor `__init__` that requires
FULL ARGUMENT BINDING (a variadic `*args`/`**kwargs` parameter, or a
keyword-only parameter) must propagate out of the `ClassName(...)` construct
expression — exactly as CPython does.

Regression for task #60 (P0 silent control-flow elision / silent exception
swallow, native). Root cause (runtime), two parts in
runtime/molt-runtime/src/call/bind.rs:

  1. `call_bind_ic_dispatch` (the entry point the compiled `call_bind` op
     lowers to) ran `call_bind_ic_entry_for_call` AFTER the call returned, to
     populate the per-site inline cache. For a constructor that path performs
     `__new__`/`__init__` MRO attribute probes which reset the exception-pending
     baseline. When a full-binding `__init__` raised, the exception was correctly
     pending at that point, but the IC-entry probe CLEARED the pending flag
     before the caller's `check_exception` (which reads that flag byte) could
     observe it — so the `try` saw no exception. Fix: skip IC-cache population
     when the call left a pending exception (a raised call is not cacheable).

  2. `call_type_with_builder`'s `InitArgPolicy::ForwardArgs` arm (the
     full-binding constructor lane) returned the partially-constructed instance
     unconditionally after invoking `__init__`, instead of returning the `none`
     sentinel on a pending exception like every other constructor return path
     (the IC fast path, `call_class_init_with_args`). All post-`__init__`
     constructor returns now route through one shared helper
     (`resolve_construct_after_init`) so the lanes cannot re-diverge.

The SIMPLE (non-full-binding) `__init__` IC fast path and plain methods /
`__new__` already propagated correctly, so the swallow was specific to
full-binding `__init__`:

  * `def __init__(self, ea, **kw)`     -> was SKIPPED  (this file's `K`/`R`/`S`)
  * `def __init__(self, *args)`        -> was SKIPPED  (`A`)
  * `def __init__(self, *, ea)`        -> was SKIPPED  (`O` keyword-only)
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
