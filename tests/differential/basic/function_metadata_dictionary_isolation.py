"""Typed function fields and ordinary user metadata have independent authority."""


def target(a=11, *, b=13):
    "original documentation"
    return a, b


def other():
    return "wrong code"


original_code = target.__code__
original_globals = target.__globals__
target.__dict__.update(
    __defaults__=(101,),
    __kwdefaults__={"b": 103},
    __name__="forged",
    __qualname__="forged.qualname",
    __doc__="forged documentation",
    __code__=other.__code__,
    __globals__={},
    __closure__=("forged closure",),
)
print("dict writes", target(), target.__defaults__, target.__kwdefaults__)
print("names", target.__name__, target.__qualname__, target.__doc__)
print(
    "execution fields",
    target.__code__ is original_code,
    target.__globals__ is original_globals,
    target.__closure__ is None,
)
target.__defaults__ = (17,)
target.__kwdefaults__ = {"b": 19}
print("typed writes", target(), target.__dict__["__defaults__"])
target.__dict__.clear()
print("dict clear", target(), target.__name__, target.__doc__)
target.__dict__ = {"__defaults__": (107,), "__kwdefaults__": {"b": 109}}
print("dict replace", target(), target.__defaults__, target.__kwdefaults__)
del target.__defaults__
del target.__kwdefaults__
print("typed delete", target.__defaults__, target.__kwdefaults__)
print("dict remains", target.__dict__["__defaults__"], target.__dict__["__kwdefaults__"])
try:
    target()
except TypeError:
    print("required arguments")
print("explicit arguments", target(23, b=29))
