"""Purpose: differential coverage for format protocol."""


class Custom:
    def __init__(self, tag):
        self.tag = tag

    def __format__(self, spec):
        return f"custom[{self.tag}:{spec}]"


class Plain:
    pass


class Box:
    def __init__(self):
        self.name = "molt"
        self.items = {"x": 2, "y": 3}
        self.seq = [10, 20]


print(f"{Custom('a'):q}")
print("{obj:q}".format(obj=Custom("b")))

try:
    print(f"{Plain():>5}")
except Exception as e:
    print(type(e).__name__, e)


# Native and Formatter paths share field grammar, while retaining their own
# callback orchestration. Print both successful projections and ordered effects.
import _string
import string
import sys


events = []


class Observed:
    def __init__(self, name):
        self.name = name

    def __format__(self, spec):
        events.append(("format", self.name, spec))
        return "<" + self.name + ":" + spec + ">"

    def __str__(self):
        events.append(("str", self.name))
        return self.name

    def __repr__(self):
        events.append(("repr", self.name))
        return "repr-" + self.name

    def __getitem__(self, key):
        events.append(("item", type(key).__name__, key))
        return self

    def __getattr__(self, name):
        events.append(("attr", name))
        return self


def supplied(name):
    events.append(("arg", name))
    return Observed(name)


def observe(label, operation):
    events.clear()
    try:
        result = operation()
        print(label, "ok", ascii(result), ascii(events))
    except Exception as error:
        print(label, type(error).__name__, ascii(str(error)), ascii(events))


observe("auto-named", lambda: "{} {x} {}".format(1, 2, x=3))
observe("named-auto", lambda: "{x} {}".format(1, x=2))
observe("nested-names", lambda: "{:{f}}{g}{}".format(1, 2, f="03", g="x"))
observe("auto-manual", lambda: "{} {0}".format(1))
observe("manual-auto", lambda: "{0} {}".format(1))
observe("decimal-fields", lambda: "{١} {०}".format("zero", "one"))
observe("nondigit-name", lambda: "{²}".format(**{"²": "name"}))
observe("decimal-item", lambda: "{0[١]} {0[²]}".format({1: "int", "²": "str"}))
observe("brace-key", lambda: "{0[{}]} {0[a!b:c]} {0[[x]}".format(
    {"{}": "braces", "a!b:c": "markers", "[x": "bracket"}
))
observe("surrogate-field", lambda: "{\ud800}".format(**{"\ud800": "\udfff"}))
observe("map-brace-key", lambda: "{x[{}]}".format_map({"x": {"{}": "value"}}))
observe("map-positional", lambda: "{}".format_map({}))

# Literal syntax failures must happen after arguments and prior fields execute.
observe("literal-trailing-open", lambda: "{0} {".format(supplied("literal")))
dynamic_bad = " ".join(("{0}", "{"))
observe("dynamic-trailing-open", lambda: dynamic_bad.format(supplied("dynamic")))
observe("literal-trailing-close", lambda: "{:}}".format(supplied("close")))
observe("lookup-before-conversion", lambda: "{0!x}".format())
observe("conversion-after-lookup", lambda: "{0[x]!x}".format(supplied("lookup")))
observe("conversion-unicode", lambda: "{0!é}".format(supplied("unicode")))
observe("conversion-surrogate", lambda: "{0!\ud800}".format(supplied("surrogate")))
observe("conversion-nul", lambda: "{0!\0}".format(supplied("nul")))
observe("conversion-syntax", lambda: "{0!rr}".format(supplied("syntax")))
observe("key-before-bad-suffix", lambda: "{0[x]bad}".format(supplied("suffix")))
observe("attr-before-empty", lambda: "{0.attr.}".format(supplied("attribute")))
observe("nested-allowed", lambda: "{0:{1}}".format(supplied("outer"), supplied("inner")))
observe("nested-limit", lambda: "{0:{1:{2}}}".format(
    supplied("outer"), supplied("inner"), supplied("deep")
))
observe("conversion-before-spec", lambda: "{0!s:{1}}".format(supplied("outer"), "^8"))
observe("escaped-spec", lambda: "{0:{{}}}".format(supplied("escaped")))
observe("lookup-surrogate", lambda: "{0[\ud800]}".format(supplied("key")))
observe("fallible-composition", lambda: ("[{0}]" * 24).format("x" * 32))


# Composition may retain only a nonempty final field's owned string, or the
# complete literal receiver. Empty output is always an exact str.
class FormatString(str):
    pass


class ReturningFormat:
    def __init__(self, result):
        self.result = result

    def __format__(self, spec):
        return self.result


saved_format_string = FormatString("abc")
saved_empty_string = FormatString("")
saved_format_value = ReturningFormat(saved_format_string)
saved_empty_value = ReturningFormat(saved_empty_string)


def composition_identity(result, expected):
    return result, result is expected, type(result).__name__, type(result) is str


observe("identity-final-field", lambda: composition_identity(
    "{}".format(saved_format_value), saved_format_string
))
observe("identity-final-spec-field", lambda: composition_identity(
    "{:s}".format(saved_format_value), saved_format_string
))
observe("identity-empty-prefix-field", lambda: composition_identity(
    "{}{}".format(saved_empty_value, saved_format_value), saved_format_string
))
observe("identity-empty-tail-field", lambda: composition_identity(
    "{}{}".format(saved_format_value, saved_empty_value), saved_format_string
))
observe("identity-empty-field", lambda: composition_identity(
    "{}".format(saved_empty_value), saved_empty_string
))
observe("identity-map-final-field", lambda: composition_identity(
    "{x}".format_map({"x": saved_format_value}), saved_format_string
))
observe("identity-map-empty-tail", lambda: composition_identity(
    "{x}{y}".format_map({"x": saved_format_value, "y": saved_empty_value}),
    saved_format_string,
))
observe("identity-map-empty-field", lambda: composition_identity(
    "{x}".format_map({"x": saved_empty_value}), saved_empty_string
))
observe("identity-literal-source", lambda: composition_identity(
    saved_format_string.format(), saved_format_string
))
observe("identity-map-literal-source", lambda: composition_identity(
    saved_format_string.format_map({}), saved_format_string
))
observe("identity-empty-source", lambda: composition_identity(
    saved_empty_string.format(), saved_empty_string
))
observe("identity-map-empty-source", lambda: composition_identity(
    saved_empty_string.format_map({}), saved_empty_string
))
escaped_format_source = FormatString("{{abc}}")
observe("identity-escaped-source", lambda: composition_identity(
    escaped_format_source.format(), escaped_format_source
))


class SpecIdentity:
    def __format__(self, spec):
        events.append(("spec-identity", spec is saved_format_string, type(spec).__name__))
        return spec


observe("identity-nested-spec", lambda: composition_identity(
    "{0:{1}}".format(SpecIdentity(), saved_format_value), saved_format_string
))
observe("identity-map-nested-spec", lambda: composition_identity(
    "{x:{y}}".format_map({"x": SpecIdentity(), "y": saved_format_value}),
    saved_format_string,
))


class TemporaryFormatString(str):
    def __str__(self):
        raise AssertionError("expanded spec must not redispatch __str__")

    def __del__(self):
        events.append(("release", "string", repr(self)))


class TemporaryFormatValue:
    def __init__(self, name):
        self.name = name

    def __format__(self, spec):
        events.append(("temporary-format", self.name, type(spec).__name__, spec))
        return TemporaryFormatString(self.name)

    def __del__(self):
        events.append(("release", "value", self.name))


class TemporaryFormatMapping:
    def __getitem__(self, key):
        events.append(("temporary-get", key))
        if key == "empty":
            return ""
        return TemporaryFormatValue(key)


observe("composition-release-order", lambda: "{value}{empty}".format_map(
    TemporaryFormatMapping()
))
observe("nested-spec-projection-release-order", lambda: "{value:{spec}}{empty}".format_map(
    TemporaryFormatMapping()
))


# Delete the callback's self local before raising so the traceback cannot defer
# the temporary owner's finalizer until after the format call has unwound.
class FailingLookupOwner:
    def __getattr__(self, name):
        events.append(("lookup", name))
        del self
        raise LookupError("format lookup sentinel")

    def __del__(self):
        events.append(("finalize", "owner"))
        raise RuntimeError("format finalizer sentinel")


class TemporaryLookupMapping:
    def __getitem__(self, key):
        events.append(("mapping", key))
        return FailingLookupOwner()


def failing_lookup_cleanup():
    previous_hook = sys.unraisablehook

    def capture_unraisable(error):
        events.append(("unraisable", error.exc_type.__name__, str(error.exc_value)))

    sys.unraisablehook = capture_unraisable
    try:
        return "{x.missing}".format_map(TemporaryLookupMapping())
    finally:
        sys.unraisablehook = previous_hook


observe("lookup-error-survives-finalizer", failing_lookup_cleanup)


def parse_projection(text):
    iterator = _string.formatter_parser(text)
    result = []
    while True:
        try:
            result.append(next(iterator))
        except StopIteration:
            return result
        except Exception as error:
            return result, type(error).__name__, str(error)


for parser_text in (
    "", "plain", "a{{b}}c", "{}", "{0[{}]!r:{{}}}",
    "\ud800{\udfff!é:\udabc}", "{0!\0}", "{0!\ud800}",
    "{0} {", "{0} }", "{0!", "{0!r", "{0!rr}", "{x{y}}",
    "{0[missing}", "{:}}", "{0:{{}}}",
):
    observe("parser " + ascii(parser_text), lambda: parse_projection(parser_text))


def split_projection(text):
    first, rest = _string.formatter_field_name_split(text)
    result = []
    while True:
        try:
            result.append(next(rest))
        except StopIteration:
            return first, result
        except Exception as error:
            return first, result, type(error).__name__, str(error)


for field_text in (
    "", "١.attr[²][०]", "²", "0[{}][a!b:c][[x]", "\ud800.\udfff[\udabc]",
    "0[x]bad", "0.attr.", "0[", "0[]", "0..attr", "0[x]é[x]",
    "9" * 40, "9" * 40 + "x", "0[" + "9" * 40 + "]", "0[" + "9" * 40 + "x]",
):
    observe("split " + ascii(field_text), lambda: split_projection(field_text))


def boundary_projection():
    first, rest = _string.formatter_field_name_split(str(sys.maxsize) + "[" + str(sys.maxsize) + "]")
    step = next(rest)
    return first == sys.maxsize, step == (False, sys.maxsize), list(rest)


observe("index-boundary", boundary_projection)
observe("index-overflow", lambda: _string.formatter_field_name_split(str(sys.maxsize + 1)))
observe("native-index-overflow", lambda: ("{" + "9" * 40 + "}").format())
observe("native-key-overflow", lambda: ("{0[" + "9" * 40 + "]}").format(Observed("index")))
observe("parser-type", lambda: _string.formatter_parser(1))
observe("split-type", lambda: _string.formatter_field_name_split(1))


class RecordingFormatter(string.Formatter):
    def get_value(self, key, args, kwargs):
        events.append(("get", key))
        return super().get_value(key, args, kwargs)

    def format_field(self, value, spec):
        events.append(("formatter", spec))
        return super().format_field(value, spec)


formatter = RecordingFormatter()
observe("Formatter-lazy-markup", lambda: formatter.format("{0} {", supplied("formatter")))
observe("Formatter-lazy-lookup", lambda: formatter.format("{0[x]bad}", supplied("formatter")))
observe("Formatter-auto-name", lambda: formatter.format("{} {x}", 1, x=2))
observe("Formatter-keys", lambda: formatter.format("{0[{}]} {0[١]}", {"{}": "brace", 1: "one"}))
observe("Formatter-overflow", lambda: formatter.format("{0[" + "9" * 40 + "]}", Observed("index")))
observe("Formatter-auto-then-lookup", lambda: formatter.format("{} {0.real}", 1))
observe("Formatter-empty-lookup-head", lambda: formatter.format("{.real}", 1))
observe("Formatter-nondecimal-digit-name", lambda: formatter.format("{} {²}", 1, **{"²": 2}))


class FieldOverrideFormatter(string.Formatter):
    def get_field(self, field_name, args, kwargs):
        events.append(("get-field-override", field_name))
        return "override", field_name


observe("Formatter-override-before-index-overflow", lambda: FieldOverrideFormatter().format(
    "{99999999999999999999}"
))


# The receiver's actual .format callable is captured before argument evaluation.
class FormatReceiver(str):
    def format(self, value):
        return "original:" + value


def replace_format():
    FormatReceiver.format = lambda self, value: "replacement:" + value
    return "argument"


receiver = FormatReceiver("{}")
observe("captured-format-callable", lambda: receiver.format(replace_format()))
observe("subsequent-format-callable", lambda: receiver.format("later"))

box = Box()
print("{0.name} {0.items[x]} {0.seq[1]}".format(box))
print("{box.name} {box.items[y]} {box.seq[0]}".format(box=box))

print("{:n}".format(1234567))
print("{:n}".format(12345.67))
try:
    print("{:,n}".format(123))
except Exception as e:
    print(type(e).__name__, e)

spec = ">6"
print(f"{3:{spec}}")
width = 4
prec = 2
print(f"{12.345:{width}.{prec}f}")

print(format(7))
print(format(7, ">4"))
print(format(Custom("c"), "z"))
try:
    print("{:_n}".format(123))
except Exception as e:
    print(type(e).__name__, e)
