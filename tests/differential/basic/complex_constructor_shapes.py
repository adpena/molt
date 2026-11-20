"""complex() argument shapes: CPython 3.14 routes a lone positional argument
through actual_complex_new and names the real/imag arguments in its errors."""

import warnings
ev = []
class C(float):
    def __float__(self):
        ev.append("f"); return 9.5
class I(int):
    def __float__(self):
        ev.append("i"); return 7.5
class D(float):
    def __complex__(self):
        ev.append("c"); return 3j
class CX(complex):
    pass
class OnlyComplex:
    def __complex__(self):
        return 2j
def show(label, thunk):
    ev.clear()
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        try:
            value = thunk()
            print(label, value, type(value).__name__, ev)
        except Exception as exc:
            print(label, type(exc).__name__, exc)
        for w in caught:
            print("  warn", w.category.__name__, w.message)
show("pos-float-sub", lambda: complex(C(1.25)))
show("kw-float-sub", lambda: complex(real=C(1.25)))
show("imag-float-sub", lambda: complex(1, C(1.25)))
show("both-float-sub", lambda: complex(C(1.25), C(2.5)))
show("pos-int-sub", lambda: complex(I(3)))
show("pos-dunder", lambda: complex(D(1.0)))
show("complex-real-imag", lambda: complex(1j, C(1.25)))
show("complex-imag", lambda: complex(1, 2j))
show("only-complex-imag", lambda: complex(OnlyComplex(), 1))
show("pos-list", lambda: complex([]))
show("kw-list", lambda: complex(real=[]))
show("imag-list", lambda: complex(1, []))
show("pos-sub", lambda: complex(CX(1, 2)))
show("pos-exact", lambda: complex(1+2j))
show("pos-none", lambda: complex(None))
show("pos-str", lambda: complex("1+2j"))
show("type-sub", lambda: CX(C(1.25)))
show("type-sub-kw", lambda: CX(real=C(1.25)))
show("kw-exact-complex", lambda: complex(real=1j))
show("kw-only-complex", lambda: complex(real=OnlyComplex()))
show("pos-only-complex", lambda: complex(OnlyComplex()))
show("sub-real-imag", lambda: complex(CX(1, 2), 0))
show("str-imag", lambda: complex("1", 2))
show("imag-str", lambda: complex(1, "2"))
show("no-args", lambda: complex())
