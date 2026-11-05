"""Proved builtin reads in a class body keep their named lowering (HF-776).

No code here writes a builtins name, and the class mapping is a plain dict, so
the binding flow proves each read until the first call in the body. The
frontend lowers those reads by identity, and the proved `locals()` call reads
the class mapping. Reads after the first call keep the live lookup, as on
main.
"""


class Body:
    x = 1
    error = ValueError
    marker = NotImplemented
    kind_type = type
    names = sorted(locals())
    kind = type(x)
    made = KeyError("k")

    def _get(self):
        return 3

    prop = property(_get)
    static = staticmethod(len)


print(Body.names)
print(Body.kind_type, Body.kind, Body.error, Body.made, Body.marker)
print(Body().prop, Body.static("ab"))
