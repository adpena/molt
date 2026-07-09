"""Purpose: differential coverage for the keyword-call / trampoline binder lane.

Regression guard for the silent-wrong-answer miscompile where a call site with a
keyword argument (or a callee with a keyword-only parameter) was dispatched through
the variadic trampoline lane but invoked the callee's FIXED-ARITY entry with the
trampoline ABI ``fn(closure, argv_ptr, argc)``. That reinterpreted ``closure`` (0)
and the raw ``argv`` pointer as the first two positional parameters, so every
argument was silently replaced by junk NaN-box bits -- e.g. ``f(1, d=4)`` returned
``(0.0, <argv-ptr-as-f64>)`` instead of ``(1, 4)``.

Positional-only call sites never routed through that lane, so this file deliberately
exercises the keyword / keyword-only / default-binding shapes that DO. Warm
default-root caches previously masked the regression; keep this file lean and pure so
it recompiles cold and can never be masked again.
"""


# --- plain positional callee, keyword call site (the minimal reproducer) ---
def plain(a, d):
    return (a, d)


print(plain(1, 2))
print(plain(1, d=4))
print(plain(a=1, d=4))
print(plain(d=4, a=1))


# --- keyword-only parameter (forces the binder even for the required arg) ---
def kwonly(a, *, d):
    return (a, d)


print(kwonly(1, d=4))
print(kwonly(a=1, d=4))


# --- keyword-only parameter WITH default: even a plain positional call binds ---
def kwonly_default(a, *, d=9):
    return (a, d)


print(kwonly_default(1))
print(kwonly_default(1, d=4))


# --- positional default, no keyword at the site (baseline that must stay correct) ---
def posdefault(a, b=2):
    return (a, b)


print(posdefault(1))
print(posdefault(1, 5))
print(posdefault(1, b=5))


# --- positional-only marker plus a keyword tail ---
def posonly(a, /, b, *, c):
    return (a, b, c)


print(posonly(1, 2, c=3))
print(posonly(1, b=2, c=3))


# --- higher arity: stress the trampoline argv unpacking (>2 slots) ---
def wide(a, b, c, d, e):
    return (a, b, c, d, e)


print(wide(1, 2, 3, 4, 5))
print(wide(1, 2, 3, d=4, e=5))
print(wide(1, e=5, c=3, d=4, b=2))


# --- fully mixed: posonly + positional-default + kw-only + kw-only-default ---
def mixed(a, b=2, /, c=3, *, d, e=5):
    return (a, b, c, d, e)


print(mixed(1, d=4))
print(mixed(1, 2, 3, d=4, e=6))


# --- floats must still round-trip as floats through the same lane (not corrupted) ---
def floaty(a, d):
    return (a, d)


print(floaty(1.5, d=2.25))
print(floaty(a=0.0, d=1.0))
