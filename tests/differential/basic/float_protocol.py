"""Purpose: differential coverage for float protocol."""

import sys
import warnings


class Floaty:
    def __float__(self):
        return 1.25


class Indexy:
    def __index__(self):
        return 7


print(float(Floaty()), float(Indexy()))


class BadFloat:
    def __float__(self):
        return 1


try:
    float(BadFloat())
except Exception as e:
    print(e)


class BadIndex:
    def __index__(self):
        return 1.5


try:
    float(BadIndex())
except Exception as e:
    print(e)


print(pow(2, 5, 7))
print(pow(3, -1, 11))

try:
    pow(2.0, 3, 5)
except Exception as e:
    print(e)

try:
    pow(2, 3, 0)
except Exception as e:
    print(e)

try:
    pow(2, -1, 4)
except Exception as e:
    print(e)


# One numeric protocol, with distinct constructor/payload-first entry policies.


events = []


class FloatSubclass(float):
    def __float__(self):
        events.append("float-subclass")
        return 2.5


class IntSubclass(int):
    def __float__(self):
        events.append("int-subclass")
        return 3.5

    def __index__(self):
        events.append("int-index")
        return 9


class IndexIntSubclass(int):
    def __index__(self):
        events.append("ignored-int-index")
        raise AssertionError("inherited int float conversion reads payload")


class NumericText(str):
    def __float__(self):
        events.append("text-float")
        return 8.5


class FloatAndIndex:
    def __float__(self):
        events.append("float")
        return 4.5

    def __index__(self):
        events.append("index")
        return 9


class RaiseFloat(FloatAndIndex):
    def __float__(self):
        events.append("raise-float")
        raise RuntimeError("float sentinel")


class SpecialLookup(FloatAndIndex):
    def __getattribute__(self, name):
        events.append("getattribute")
        raise AssertionError("numeric special lookup used instance attributes")


class FloatSlot:
    def __get__(self, obj, owner):
        events.append("bind-float")
        return lambda: 5.25


class DescriptorFloat:
    __float__ = FloatSlot()


class RaiseIndex:
    def __index__(self):
        events.append("raise-index")
        raise LookupError("index sentinel")


class InvalidFloat(FloatAndIndex):
    def __float__(self):
        events.append("invalid-float")
        return 1


class InvalidIndex:
    def __index__(self):
        events.append("invalid-index")
        return 1.5


class FloatReturnSubclass:
    def __float__(self):
        events.append("return-float-subclass")
        return FloatSubclass(6.25)


class IndexReturnSubclass:
    def __index__(self):
        events.append("return-int-subclass")
        return IntSubclass(6)


class IndexReturnBool:
    def __index__(self):
        events.append("return-bool")
        return True


class HugeIndex:
    def __index__(self):
        events.append("huge-index")
        return 1 << 4096


class InstanceOnly:
    pass


instance_only = InstanceOnly()
instance_only.__float__ = lambda: 10.5
instance_only.__index__ = lambda: 10


def memoryview_number(value):
    view = memoryview(bytearray(8)).cast("d")
    try:
        view[0] = value
        return view[0]
    finally:
        view.release()


operations = [
    ("float", float),
    ("percent", lambda value: "%f" % value),
    ("memoryview", memoryview_number),
]
if hasattr(float, "from_number"):
    operations.append(("from-number", float.from_number))


def conversion_result(label, value, action):
    for name, operation in operations:
        events.clear()
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter(action, DeprecationWarning)
            try:
                result = operation(value)
                outcome = ("ok", repr(result))
            except Exception as exc:
                outcome = (type(exc).__name__, str(exc))
        print("conversion", label, name, action, outcome, events)
        print(
            "warnings", [(item.category.__name__, str(item.message)) for item in caught]
        )


for label, value in (
    ("float-subclass", FloatSubclass(1.25)),
    ("int-subclass", IntSubclass(1)),
    ("int-index-override", IndexIntSubclass(7)),
    ("numeric-text", NumericText("not a float")),
    ("both", FloatAndIndex()),
    ("special-lookup", SpecialLookup()),
    ("float-descriptor", DescriptorFloat()),
    ("raise-float", RaiseFloat()),
    ("raise-index", RaiseIndex()),
    ("invalid-float", InvalidFloat()),
    ("invalid-index", InvalidIndex()),
    ("instance-only", instance_only),
    ("huge-int", 1 << 4096),
    ("huge-index", HugeIndex()),
    ("text", "1.25"),
    ("bytes", b"1.25"),
    ("nan", float("nan")),
    ("infinity", float("inf")),
):
    conversion_result(label, value, "always")

for label, value in (
    ("float-return-subclass", FloatReturnSubclass()),
    ("index-return-subclass", IndexReturnSubclass()),
    ("index-return-bool", IndexReturnBool()),
):
    for action in ("always", "ignore", "error"):
        conversion_result(label, value, action)

# The runtime warning also reaches custom display hooks, without a private
# deduplication cache suppressing repeated "always" warnings.
with warnings.catch_warnings():
    warnings.simplefilter("always", DeprecationWarning)
    old_showwarning = warnings.showwarning
    shown = []

    def showwarning(message, category, filename, lineno, file=None, line=None):
        shown.append((category.__name__, str(message)))

    warnings.showwarning = showwarning
    try:
        float(FloatReturnSubclass())
        float(FloatReturnSubclass())
    finally:
        warnings.showwarning = old_showwarning
    print("custom-warning", shown)

# Explicit base descriptors inspect intrinsic storage, bypassing __float__.
for label, payload in (
    ("fraction", 1.25),
    ("negative-zero", -0.0),
    ("subnormal", float.fromhex("0x0.0000000000001p-1022")),
    ("nan", float("nan")),
    ("infinity", float("inf")),
):
    value = FloatSubclass(payload)
    events.clear()
    try:
        ratio = float.as_integer_ratio(value)
    except (ValueError, OverflowError) as error:
        ratio = (type(error).__name__, str(error))
    print(
        "float-descriptors", label,
        float.hex(float.__float__(value)),
        float.hex(float.conjugate(value)),
        float.is_integer(value), float.hex(value), ratio, events[:],
    )

for descriptor in (
    float.__float__, float.conjugate, float.is_integer,
    float.as_integer_ratio, float.hex,
):
    events.clear()
    try:
        descriptor(FloatAndIndex())
    except TypeError as error:
        print("float-descriptor-rejects-protocol", type(error).__name__, events[:])

# Formatting must use the sealed numeric storage while keeping each format's
# conversion and override rules. In particular, an int floating-point format
# invokes __float__, whereas integer formats and float-subclass formats do not.
for label, value in (
    ("int-subclass", IntSubclass(42)),
    ("int-huge-subclass", IntSubclass(1 << 4096)),
    ("int-index-override", IndexIntSubclass(42)),
    ("float-subclass", FloatSubclass(1.25)),
    ("float-negative-zero", FloatSubclass(-0.0)),
    ("float-infinity", FloatSubclass(float("inf"))),
):
    for spec in ("", "d", "#x", ".2f", "+.3g"):
        events.clear()
        try:
            outcome = ("ok", format(value, spec))
        except Exception as error:
            outcome = (type(error).__name__, str(error))
        print("scalar-format", label, spec, outcome, events[:])


class RenderInt(int):
    def __repr__(self):
        return "int-repr"

    def __str__(self):
        return "int-str"


class RenderFloat(float):
    def __repr__(self):
        return "float-repr"

    def __str__(self):
        return "float-str"


for value in (RenderInt(42), RenderFloat(1.25)):
    print("scalar-render-overrides", str(value), repr(value), format(value, ""),
          format(value, ".2f"))

# Rounded decimal notation, padding and PEP 682 share the numeric renderer.
for value, spec in (
    (1.5, "<08"), (-1.5, "=08"), (float("inf"), ">06"),
    ("ab", "<05"), ("ab", "=05"), ("ab", "x>05"),
    (-0.0, "z"), (-0.04, "z.1f"), (-0.06, "+z.1f"),
    (-0.0004, "z.1%"), (0, "z"), ("ab", "z"),
    (10.0, ".3"), (100.0, ".3"), (2.0, ".2"),
    (999999.7, "g"), (0.0000999999999, "g"),
    (999999.7, "#g"), (0.0, "#g"), (0.0, ".1"), (1e16, "#"),
    (1.0, ".2147483648f"),
    (complex(1, -float("nan")), ""),
    (complex(-float("nan"), 1), "+"),
    (complex(-0.0, -0.0), "z"), (complex(-0.04, -0.04), "+z.1f"),
    (complex(10, 100), ".3"), (complex(1, 2), "x>08"),
    (complex(1, 2), "#"), (complex(1e16, 2), "#"),
):
    try:
        outcome = ("ok", format(value, spec))
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("format-boundary", repr(value), spec, outcome)
print("percent-rounding", "%g" % 999999.7, "%g" % 0.0000999999999)
print("complex-nan-repr", repr(complex(1, -float("nan"))))

# Source calls and first-class calls share constructor binding and evaluation.
def memoryview_argument():
    events.append("memoryview-argument")
    return b"xy"


for label, operation in (
    ("keyword", lambda: memoryview(object=b"xy").tobytes()),
    ("missing", lambda: memoryview()),
    ("nonbuffer", lambda: memoryview(1)),
    ("unknown", lambda: memoryview(obj=b"xy")),
    ("too-many", lambda: memoryview(b"xy", memoryview_argument())),
    ("duplicate", lambda: memoryview(b"xy", object=memoryview_argument())),
    ("unknown-extra", lambda: memoryview(object=b"xy", extra=memoryview_argument())),
    ("str-duplicate", lambda: str(42, object=42)),
    ("complex-keyword", lambda: complex(extra=1)),
):
    events.clear()
    try:
        outcome = ("ok", operation())
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("constructor-boundary", label, outcome, events[:])

# The shared grammar owns diagnostics before receiver-specific presentation.
class RenderComplex(complex):
    pass


for value, spec in (
    (1.5, ",s"), (1, ",s"), (1j, ",x"), ("a", ",x"),
    (1.5, ",_"), (1.5, "_,"), (1.5, ".2d"), (1.5, "zd"),
    (1.5, "+c"), (1.5, "#c"), (5, "z.1"), ("ab", "z#"),
    (1.5, "ff"), (1.5, "."), (1.5, ".f"),
    (1.5, "é"), (1.5, "\ud800"), (1.5, "\ud800f"),
    (1.5, "١٠"), (1.5, ".٢f"), (1.5, "𑁦𑁧𑁦"),
    (1, "\x00"), (1.5, "\x00"), (1j, "\x00"), ("a", "\x00"),
    (1.5, "999999999999999999999999999999999999f"),
    (1.5, ".999999999999999999999999999999999999f"),
    (1e308, "#%"), (1e308, "010,%"), (-1e308, "010,%"),
    (1j, "0.2147483648"), (RenderComplex(1), "d"),
):
    try:
        outcome = ("ok", format(value, spec))
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("format-parser-boundary", type(value).__name__, repr(spec), outcome)

for spec, arguments in (
    ("%.3000000000f", FloatAndIndex()),
    ("%.3000000000s", FloatAndIndex()),
    ("%999999999999999999999999999999d", 5),
    ("%*d", (FloatAndIndex(), 5)),
    ("%.*f", (FloatAndIndex(), 1.5)),
    ("%.*f", (2147483648, 1.5)),
    ("%*d", (1 << 200, 5)),
    ("%.*f", (1 << 200, 1.5)),
    ("%.*f", (-1, 1.5)),
    ("%*s", (-sys.maxsize - 1, "x")),
    ("%*d", (-sys.maxsize - 1, 5)),
    ("%hhf", 1.5), ("%llf", 1.5), ("%hf", 1.5), ("%lf", 1.5),
):
    events.clear()
    try:
        outcome = ("ok", spec % arguments)
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("percent-parser-boundary", spec, outcome, events[:])

events.clear()
try:
    outcome = ("ok", format(IntSubclass(42), ".2147483648f"))
except Exception as error:
    outcome = (type(error).__name__, str(error))
print("format-conversion-error-order", outcome, events[:])

class PercentMapping:
    def __getitem__(self, key):
        events.append(("percent-getitem", key))
        if key == "missing":
            raise LookupError("mapping lookup failed")
        return 3 if key == "width" else "mapped"


for spec in ("%(a(b)c)s", "%(key).3000000000f", "%(missing).3000000000f",
             "%(width)*s", "%(key)s/%s", "%(key)"):
    events.clear()
    try:
        outcome = ("ok", spec % PercentMapping())
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("percent-mapping-order", spec, outcome, events[:])

# Calls through imported aliases and live builtins keep the actual callee.
import builtins
from builtins import memoryview as imported_memoryview
from builtins import pow as imported_pow

print("constructor-import-alias", imported_memoryview(b"xy").tobytes(),
      imported_pow(base=2, exp=3))

from builtins import round as pow, pow as p
from builtins import int as memoryview, memoryview as view
print("constructor-deceptive-aliases", p(2, 3), view(b"ab").tobytes())
pow = builtins.pow
memoryview = builtins.memoryview
for label, operation in (
    ("str-duplicate-order", lambda: str(1, extra=2, object=3)),
    ("bytes-duplicate-order", lambda: bytes(1, extra=2, source=3)),
    ("bytearray-duplicate-order", lambda: bytearray(1, extra=2, source=3)),
    ("pow-missing", lambda: pow(2)),
    ("pow-empty", lambda: pow()),
    ("pow-too-many", lambda: pow(1, 2, 3, 4)),
    ("pow-keywords", lambda: pow(base=2, exp=3)),
    ("int-new-bool-base", lambda: int.__new__(bool, base=10)),
    ("int-new-bool-unknown", lambda: int.__new__(bool, extra=1)),
    ("int-new-bool-count", lambda: int.__new__(bool, 1, 2, 3)),
    ("memoryview-new-missing", lambda: memoryview.__new__()),
    ("memoryview-new-instance", lambda: memoryview.__new__(1)),
    ("memoryview-new-wrong-type", lambda: memoryview.__new__(bytes, b"xy")),
    ("memoryview-order", lambda: memoryview(b"xy").tobytes(order="C")),
    ("memoryview-order-invalid", lambda: memoryview(b"xy").tobytes(order="invalid")),
    ("memoryview-order-effect", lambda: memoryview(b"xy").tobytes(order=memoryview_argument())),
):
    events.clear()
    try:
        outcome = ("ok", operation())
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("constructor-callee-boundary", label, outcome, events[:])

for label, view in (
    ("matrix", memoryview(b"abcdefghijkl").cast("B", (3, 4))),
    ("stride", memoryview(b"abcdefghijkl")[::2]),
    ("negative-stride", memoryview(b"abcdefghijkl")[::-2]),
):
    for order in (None, "C", "F", "A"):
        print("memoryview-order-traversal", label, order, view.tobytes(order=order))
    view.release()
    for order in (1, "invalid", "\x00", "\ud800"):
        try:
            outcome = ("ok", view.tobytes(order=order))
        except Exception as error:
            outcome = (type(error).__name__, str(error))
        print("memoryview-release-order", label, repr(order), outcome)

original_memoryview = builtins.memoryview
original_pow = builtins.pow
original_round = builtins.round
try:
    builtins.memoryview = lambda value: ("rebound-memoryview", value)
    builtins.pow = lambda *args, **kwargs: ("rebound-pow", args, kwargs)
    builtins.round = lambda *args, **kwargs: ("rebound-round", args, kwargs)
    live_view = memoryview(b"xy")
    if isinstance(live_view, original_memoryview):
        live_view = (type(live_view).__name__, live_view.tobytes())
    print("live-builtin-callee", live_view, pow(2, 3), round(1.5))
finally:
    builtins.memoryview = original_memoryview
    builtins.pow = original_pow
    builtins.round = original_round

# Fixed named Clinic builtins bind the captured callable and retain original
# keyword objects for the later unexpected-key membership/diagnostic phase.
from builtins import open as clinic_open, __import__ as clinic_import

clinic_pow = builtins.pow
clinic_round = builtins.round
for label, operation in (
    ("pow-none", lambda: clinic_pow(2, 3, None)),
    ("pow-modulus", lambda: clinic_pow(base=2, exp=5, mod=7)),
    ("pow-required-first", lambda: clinic_pow(unknown=2)),
    ("pow-missing-before-duplicate", lambda: clinic_pow(2, unknown=3, base=4)),
    ("pow-keyword-count", lambda: clinic_pow(a=1, b=2, c=3, d=4)),
    ("round-empty", lambda: clinic_round()),
    ("round-excess", lambda: clinic_round(1, 2, 3)),
    ("round-required-first", lambda: clinic_round(unknown=1)),
    ("round-duplicate", lambda: clinic_round(1, number=2)),
    ("round-count-first", lambda: clinic_round(1, unknown=2, number=3)),
    ("round-default", lambda: clinic_round(number=1.5)),
    ("round-none", lambda: clinic_round(number=1.5, ndigits=None)),
    ("open-empty", lambda: clinic_open()),
    ("open-excess", lambda: clinic_open(1, 2, 3, 4, 5, 6, 7, 8, 9)),
    ("open-required-first", lambda: clinic_open(unknown=1)),
    ("open-duplicate-first", lambda: clinic_open(None, unknown=1, file=None)),
    ("import-empty", lambda: clinic_import()),
    ("import-excess", lambda: clinic_import(1, 2, 3, 4, 5, 6)),
    ("import-required-first", lambda: clinic_import(unknown=1)),
    ("import-duplicate-first", lambda: clinic_import("builtins", unknown=1, name="builtins")),
    ("import-defaults", lambda: clinic_import(name="builtins") is builtins),
):
    try:
        outcome = ("ok", operation())
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("clinic-callee-boundary", label, outcome)


class ClinicKeyword(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        events.append(("keyword-eq", other))
        return False

    def __str__(self):
        events.append("keyword-str")
        return "displayed-key"


class ClinicAcceptedKeyword(ClinicKeyword):
    __hash__ = str.__hash__

    def __eq__(self, other):
        events.append(("keyword-accept", other))
        return True


class ClinicRaisingKeyword(ClinicKeyword):
    __hash__ = str.__hash__

    def __eq__(self, other):
        events.append(("keyword-raise", other))
        builtins.pow = lambda *args, **kwargs: "rebound-inside-keyword"
        raise LookupError("keyword membership sentinel")


class ClinicRaisingTextKeyword(ClinicKeyword):
    def __str__(self):
        raise ArithmeticError("keyword rendering sentinel")


class ClinicSurrogateKeyword(ClinicKeyword):
    def __str__(self):
        return "\ud800"


for label, operation in (
    ("matching-contents", lambda: clinic_pow(2, **{ClinicKeyword("exp"): 3})),
    ("method-matching-contents", lambda: memoryview(b"xy").tobytes(**{ClinicKeyword("order"): "C"})),
    ("unknown-membership", lambda: clinic_pow(2, 3, **{ClinicKeyword("odd"): 4})),
    ("unknown-accepts", lambda: clinic_pow(2, 3, **{ClinicAcceptedKeyword("odd"): 4})),
    ("unknown-raises", lambda: clinic_pow(2, 3, **{ClinicRaisingKeyword("odd"): 4})),
    ("rendering-raises", lambda: clinic_pow(2, 3, **{ClinicRaisingTextKeyword("odd"): 4})),
    ("rendering-surrogate", lambda: clinic_pow(2, 3, **{ClinicSurrogateKeyword("odd"): 4})),
    ("consumed-key-membership", lambda: clinic_pow(2, **{ClinicKeyword("exp"): 3, "odd": 4})),
    ("suggest-original-key", lambda: clinic_pow(2, 3, **{ClinicKeyword("modd"): 4})),
    ("surrogate-source-key", lambda: clinic_round(1, **{"ndigits\ud800": 4})),
    ("surrogate-source-display", lambda: clinic_round(1, **{ClinicKeyword("ndigits\ud800"): 4})),
    ("surrogate-source-raises", lambda: clinic_round(1, **{ClinicRaisingTextKeyword("ndigits\ud800"): 4})),
    ("method-surrogate-source", lambda: memoryview(b"xy").tobytes(**{ClinicKeyword("order\ud800"): "C"})),
    ("constructor-surrogate-source", lambda: str(**{ClinicKeyword("encoding\ud800"): "utf-8"})),
    ("round-membership", lambda: clinic_round(1, **{ClinicKeyword("odd"): 4})),
    ("open-membership", lambda: clinic_open(None, **{ClinicKeyword("odd"): 4})),
    ("import-membership", lambda: clinic_import("builtins", **{ClinicKeyword("odd"): 4})),
    ("str-dictionary-lookup", lambda: str(**{ClinicKeyword("object"): 4})),
    ("property-dictionary-lookup", lambda: property(**{ClinicKeyword("doc"): "text"})),
    ("module-dictionary-lookup", lambda: type(builtins)(**{ClinicKeyword("name"): "clinic_demo"})),
    ("memoryview-dictionary-lookup", lambda: memoryview(**{ClinicKeyword("object"): b"x"})),
):
    events.clear()
    try:
        outcome = ("ok", operation())
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    finally:
        builtins.pow = clinic_pow
    print("clinic-keyword-protocol", label, outcome, events[:], clinic_pow(2, 3))

# Large valid precision must stay inside Python rather than panicking in Rust's
# bounded formatting argument representation. Preserve all nonzero digits in
# the gold output without emitting tens of thousands of predictable zeros.
for value in (0.1, 1.0, 5e-324):
    for spec in (".65536f", ".65536e", ".65537g", "#.65537g"):
        text = format(value, spec)
        print("large-float-precision", repr(value), spec, len(text), text[:1100], text[-12:], text.count("0"))
for spec in ("%.65536f", "%.65536e", "%.65537g", "%#.65537g"):
    text = spec % 0.1
    print("large-percent-precision", spec, len(text), text[:1100], text[-12:], text.count("0"))
text = format(1j, ".65536f")
print("large-complex-precision", len(text), text[:12], text[-12:], text.count("0"))

for value in (-1, 0x110000, 1 << 40, 1 << 200):
    try:
        outcome = ("ok", format(value, "c"))
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("integer-char-range", value, outcome)

for spec in (",é", ", ", ",\ud800"):
    try:
        outcome = ("ok", format(1, spec))
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("grouping-code-display", repr(spec), outcome)

class FormatString(str):
    def __format__(self, spec):
        return "string-override:" + spec

class FormatBytes(bytes):
    def __format__(self, spec):
        return "bytes-override:" + spec

class FormatBytearray(bytearray):
    def __format__(self, spec):
        return "bytearray-override:" + spec

class FormatComplex(complex):
    def __format__(self, spec):
        return "complex-override:" + spec

for value in (FormatString("abc"), FormatBytes(b"abc"), FormatBytearray(b"abc"), FormatComplex(1j)):
    print("native-format-override", format(value, "x"), f"{value}", "{:x}".format(value))

class InheritedString(str):
    pass

for value in ("", InheritedString(""), "abc", InheritedString("abc")):
    for spec in ("s", "3", "<3", ".3", ".2", ">4"):
        text = format(value, spec)
        print("string-format-identity", type(value).__name__, spec, type(text).__name__, text is value, repr(text))

large_string = "é\ud800x" * 32768
for spec in (".0", ".1", ".2", ">5.2", "\ud800^6.1"):
    text = format(large_string, spec)
    print("string-prefix-format", repr(spec), len(text), repr(text))
for spec in ("%.0s", "%.1s", "%5.2s", "%-5.2s", "%.0r", "%.1a"):
    text = spec % large_string
    print("string-prefix-percent", spec, len(text), repr(text))

class PercentStringResult:
    def __str__(self):
        print("percent-string-callback", "str")
        return InheritedString(large_string)
    def __repr__(self):
        print("percent-string-callback", "repr")
        return InheritedString(large_string)

for spec in ("%.0s", "%5.2s", "%.0r", "%.1a"):
    print("percent-owned-prefix", spec, repr(spec % PercentStringResult()))

identity_string = InheritedString("abc")
class IdentityFormatResult:
    def __str__(self):
        return identity_string
    def __repr__(self):
        return identity_string
    def __format__(self, spec):
        return identity_string

for spec in ("%s", "%.3s", "%.2s", "%5s", "%r", "%a", "%+s", "% s", "%+r", "% a", "%#s", "%0s", "%-s", "X%s", "%sX"):
    text = spec % IdentityFormatResult()
    print("percent-result-identity", spec, type(text).__name__, text is identity_string, repr(text))
for spec in ("{}", "{:s}", "X{}", "{}X", "{}{}"):
    text = spec.format(IdentityFormatResult(), "")
    print("brace-result-identity", spec, type(text).__name__, text is identity_string, repr(text))
for value in (InheritedString("abc"), InheritedString("")):
    text = value % ()
    print("percent-literal-identity", len(value), type(text).__name__, text is value)

class CharErrorOuter:
    class Bad:
        pass
class ModuleCharError:
    pass
ModuleCharError.__module__ = "sample"
class EmptyModuleCharError:
    pass
EmptyModuleCharError.__module__ = ""

for value in ("", "ab", "é\ud800", object(), CharErrorOuter.Bad(), ModuleCharError(), EmptyModuleCharError()):
    try:
        outcome = ("ok", "%c" % value)
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("percent-char-admission", type(value).__name__, outcome)

class PercentTypeFailure:
    def __int__(self):
        raise TypeError("int callback sentinel")
    def __index__(self):
        raise TypeError("index callback sentinel")

class PercentOtherFailure:
    def __int__(self):
        raise LookupError("int callback sentinel")
    def __index__(self):
        raise LookupError("index callback sentinel")

for value in (PercentTypeFailure(), PercentOtherFailure()):
    for spec in ("%d", "%x", "%c", "%.2147483647d"):
        try:
            outcome = ("ok", spec % value)
        except Exception as error:
            outcome = (type(error).__name__, str(error))
        print("percent-conversion-failure", type(value).__name__, spec, outcome)
for spec, args in (("%5*d", (3, 4)), ("%.2147483647d", 1), ("%(key)s", "abc"), ("%(key", 5)):
    try:
        outcome = ("ok", spec % args)
    except Exception as error:
        outcome = (type(error).__name__, str(error))
    print("percent-admission-boundary", spec, outcome)

if sys.version_info >= (3, 14):
    for value, spec in (
        (12345.123456, ",.6_f"), (1.123456, "._"), (1.123456, ".5_f"),
        (1.123456, ".6_f"), (1.123456, ".6_e"), (1.123456, ".6_g"),
        (1.123456, "_.6,f"), (1.123456, ".6,_f"),
        (1.123456, ".6_,f"), (1.123456, ".6_n"),
        (complex(1.123456, 2.123456), ".6_f"), ("abcd", ".3_s"),
    ):
        try:
            outcome = ("ok", format(value, spec))
        except Exception as error:
            outcome = (type(error).__name__, str(error))
        print("fractional-format-boundary", repr(value), spec, outcome)
