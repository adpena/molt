"""Public callable identity and reduction, observed independently by CPython."""

import copyreg
import binascii
import contextlib
import functools
import _functools
import _json
import math
import operator
import time
import sys


for module, names in (
    (copyreg, ("__newobj__", "__newobj_ex__", "_reconstructor", "_reduce_ex")),
    (math, ("floor", "sqrt", "perm", "gcd", "lcm", "hypot", "isclose", "prod")),
    (binascii, ("crc32",)),
    (operator, ("and_", "countOf", "inv", "invert")),
    (time, ("monotonic",)),
    (contextlib, ("closing", "aclosing", "nullcontext", "ExitStack", "AsyncExitStack", "redirect_stdout", "redirect_stderr")),
    (functools, ("singledispatch", "reduce")),
    (_json, ("make_encoder", "make_scanner")),
):
    for name in names:
        value = getattr(module, name)
        print(module.__name__, name, type(value).__name__, value.__name__, value.__qualname__, value.__module__)
        if type(value).__name__ == "builtin_function_or_method":
            print("reduce", value.__reduce__())

print("native defaults", math.perm(5), math.perm(5, 2), binascii.crc32(b"abc"),
      binascii.crc32(b"abc", 1))
print("native variadic", math.gcd(), math.gcd(12, 18, 24), math.lcm(),
      math.lcm(3, 4, 5), math.hypot(), math.hypot(3, 4))
print("native keywords", math.prod([2, 3]), math.prod([2, 3], start=4),
      math.isclose(1.0, 1.00001), math.isclose(1.0, 1.00001, rel_tol=0.001),
      math.isclose(0.0, 0.00001, abs_tol=0.001), math.isclose(a=1.0, b=1.0))
print("functools reexports", functools.reduce is _functools.reduce,
      functools.cmp_to_key is _functools.cmp_to_key, functools.partial is _functools.partial)
print("native reduce", functools.reduce(operator.add, [1, 2, 3]),
      functools.reduce(operator.add, [1, 2], 10),
      functools.reduce(lambda left, right: left, [1], None))
if sys.version_info >= (3, 14):
    print("reduce initial keyword", functools.reduce(operator.add, [1, 2], initial=10))
else:
    try:
        functools.reduce(operator.add, [1, 2], initial=10)
    except TypeError as error:
        print("reduce initial keyword", type(error).__name__)
for operation, args, keywords in (
    (math.perm, (), {"n": 5}),
    (binascii.crc32, (), {"data": b"abc"}),
    (math.gcd, (), {"integers": (1, 2)}),
    (math.prod, ([2, 3], 4), {}),
    (math.isclose, (1.0, 2.0, 0.1), {}),
    (math.prod, ([2, 3],), {"unknown": 1}),
):
    try:
        operation(*args, **keywords)
    except TypeError as error:
        print("positional native", operation.__name__, type(error).__name__)


class Closable:
    closed = False

    def close(self):
        self.closed = True


closable = Closable()
manager = contextlib.closing(closable)
with manager as entered:
    print("closing entry", entered is closable, manager.thing is closable)
print("closing exit", closable.closed)
with contextlib.nullcontext(7) as entered:
    print("nullcontext entry", entered)


class Positional:
    def __new__(cls, value):
        result = super().__new__(cls)
        result.value = value
        return result

    def __getnewargs__(self):
        return (self.value,)


class Keywords:
    def __new__(cls, *, value):
        result = super().__new__(cls)
        result.value = value
        return result

    def __getnewargs_ex__(self):
        return (), {"value": self.value}


for value in (Positional(7), Keywords(value=9)):
    for protocol in range(2, 6):
        recipe = value.__reduce_ex__(protocol)
        helper = recipe[0]
        print("recipe", type(value).__name__, protocol, helper.__name__, helper.__qualname__, helper.__module__, type(helper).__name__, helper is getattr(copyreg, helper.__name__))


# Every adapted builtin uses the existing native callable protocol, including
# hidden Python implementation metadata and non-binding class assignment.
import _signal
import signal
import select

for module, names in (
    (time, ("time", "time_ns", "sleep", "localtime", "gmtime", "strftime", "asctime", "ctime", "mktime", "tzset", "strptime", "get_clock_info")),
    (_signal, ("signal", "getsignal", "raise_signal", "alarm", "pause", "set_wakeup_fd", "valid_signals", "strsignal", "pthread_sigmask", "pthread_kill", "sigpending", "sigwait")),
    (math, ("log", "sumprod")),
    (operator, ("call", "indexOf")),
):
    for name in names:
        function = getattr(module, name)
        print("adapted", module.__name__, name, type(function).__name__, function.__name__, function.__qualname__, function.__module__, function.__reduce__())
        print("native implementation hidden", name, [hasattr(function, field) for field in ("__code__", "__globals__", "__closure__", "__defaults__", "__dict__")])

for name in ("raise_signal", "alarm", "pause", "set_wakeup_fd", "strsignal", "pthread_kill"):
    print("signal shared builtin", name, getattr(signal, name) is getattr(_signal, name))
for name in ("SIGINT", "SIG_DFL", "SIG_BLOCK"):
    value = getattr(signal, name)
    print("signal member", name, type(value).__name__, value.__module__)
if hasattr(select, "poll"):
    print("select poll identity", type(select.poll).__name__, select.poll.__name__, select.poll.__module__)
if hasattr(select, "kqueue"):
    # Selector availability follows the target OS; validate identity in each
    # supported cell without comparing a macOS-only export with WASM's surface.
    assert type(select.kqueue).__name__ == "type"
    assert select.kqueue.__name__ == "kqueue"
    assert select.kqueue.__qualname__ == "kqueue"
    assert select.kqueue.__module__ == "select"

class NativeAdapterHolder:
    operation = math.log

print("compiled builtin nonbinding", NativeAdapterHolder().operation(1), math.log(8, 2), math.sumprod((1, 2), (3, 4)))
print("operator missing consumers", operator.call(lambda value, *, increment: value + increment, 3, increment=4), operator.indexOf(iter((2, 3, 4)), 3))
print("copyreg public recipes", copyreg.pickle_complex(2 + 3j), copyreg.pickle_union(int | str)[0] is functools.reduce)

print("math fma version surface", hasattr(math, "fma"))
