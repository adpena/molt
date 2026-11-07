"""Proved builtin reads in a class body select the published objects (HF-776).

No code here writes a builtins name, and the class mapping is a plain dict, so
the binding flow proves each read until the first call in the body; the
frontend lowers those reads by identity (`BUILTIN_TYPE`, `EXCEPTION_CLASS`,
the constants). Reads after the first call keep the live lookup, as on main.
A class-body `locals()`, `vars()` or `dir()` call is not covered: Molt returns
the module namespace there on main as well (an open sibling of HF-776).
"""


class Body:
    x = 1
    error = ValueError
    marker = NotImplemented
    ellipsis = Ellipsis
    kind_type = type
    made_first = KeyError("first")
    kind = type(x)
    error_after = ValueError
    made = KeyError("k")

    def _get(self):
        return 3

    prop = property(_get)
    static = staticmethod(len)


print(Body.kind_type, Body.kind, Body.error, Body.made_first, Body.made)
print(Body.marker, Body.ellipsis, Body.error_after)
print(
    Body.kind_type is type,
    Body.error is ValueError,
    Body.error_after is ValueError,
    Body.marker is NotImplemented,
    Body.ellipsis is Ellipsis,
)
print(Body().prop, Body.static("ab"))
