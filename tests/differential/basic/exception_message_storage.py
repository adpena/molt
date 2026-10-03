"""Exception arguments stay live and formatting belongs to observation."""

events = []


class Payload:
    def __init__(self, value):
        self.value = value

    def __str__(self):
        events.append("str")
        return self.value

    def __repr__(self):
        events.append("repr")
        return "payload:" + self.value


class NamedKeyError(ValueError):
    pass


NamedKeyError.__name__ = "KeyError"


class NamedSyntaxError(ValueError):
    pass


NamedSyntaxError.__name__ = "SyntaxError"
argument = Payload("first")
for label, cls in (
    ("base", BaseException),
    ("value", ValueError),
    ("key", KeyError),
    ("import", ImportError),
    ("syntax", SyntaxError),
    ("named-key", NamedKeyError),
    ("named-syntax", NamedSyntaxError),
):
    for args in ((), (argument,)):
        events.clear()
        error = cls(*args)
        print("construct", label, len(args), events == [], error.args == args)

for cls in (ImportError, SyntaxError):
    error = cls(argument)
    print("message-field", error.msg is argument)

events.clear()
error = ValueError(argument)
print("deferred", events == [])
print("render-first", str(error), events)
argument.value = "second"
print("render-second", str(error), events)
error.args = (Payload("third"),)
print("render-rebound", str(error), events)


class Override(ValueError):
    def __str__(self):
        return "61"


print("fused-override", int(str(Override(42))))


class Raising:
    def __str__(self):
        events.append("raising")
        raise LookupError("format failed")


events.clear()
error = ValueError(Raising())
print("raising-construct", events == [])
try:
    str(error)
except LookupError as failure:
    print("raising-observe", str(failure), events)

leaf = ValueError(argument)
group = ExceptionGroup("group", [leaf])
print("group", group.message, group.exceptions[0] is leaf)


class ObjectStr(ValueError):
    __str__ = object.__str__


error = ObjectStr(42)
print("object-str", str(error))
try:
    int(str(error))
except ValueError:
    print("object-str-int", "ValueError")


class MutableRepr(ObjectStr):
    def __repr__(self):
        events.append("custom-repr")
        return self.text


error = MutableRepr(42)
error.text = "first repr"
events.clear()
print("dynamic-object-str", str(error))
error.text = "second repr"
print("dynamic-object-str", str(error), events)
events.clear()
default = object.__repr__(error)
print("explicit-object-repr", "MutableRepr object at 0x" in default, events)
print("explicit-base", BaseException.__str__(KeyError("x")),
      str(KeyError("x")), BaseException.__repr__(KeyError("x")))

# String values, including lone surrogates, pass through owned slot results.
print("surrogate-message", ascii(str(ValueError("\ud800"))))

for cls in (SyntaxError, ImportError):
    original = Payload("message")
    error = cls(original)
    error.args = ("new args",)
    print("stored-message-diverges", cls.__name__, error.msg is original, str(error))
    error.msg = "explicit message"
    print("stored-message-replaced", cls.__name__, str(error), error.args)

group = ExceptionGroup("original", [ValueError("leaf")])
group.args = ("changed", ())
print("group-diverges", group.message, str(group))


class RebindPayload:
    def __init__(self):
        self.owner = None

    def __str__(self):
        self.owner.args = ("after str",)
        return "before str"

    def __repr__(self):
        self.owner.args = ("after repr",)
        return "before repr"


payload = RebindPayload()
error = ValueError(payload)
payload.owner = error
print("callback-args-str", str(error), str(error))
error.args = (payload, "old second")
print("callback-args-repr", repr(error), repr(error))


class BrokenDescriptor:
    def __get__(self, obj, owner):
        events.append("descriptor")
        raise LookupError("descriptor failure")


class DescriptorError(ValueError):
    __str__ = BrokenDescriptor()


class NonStringError(ValueError):
    def __str__(self):
        events.append("nonstring")
        return 7


for cls in (DescriptorError, NonStringError):
    events.clear()
    try:
        str(cls("argument"))
    except (LookupError, TypeError) as failure:
        print("slot-failure", type(failure).__name__, str(failure), events)


class ContainerObjectRepr(list):
    __repr__ = object.__repr__


class ContainerObjectStr(list):
    __str__ = object.__str__

    def __repr__(self):
        return "container repr"


print("container-default", "ContainerObjectRepr object at 0x" in repr(ContainerObjectRepr([1])))
print("container-str", str(ContainerObjectStr([1])))
print("inherited-container-repr", repr(type("TupleChild", (tuple,), {})([1, 2])))

for cls in (OSError, SyntaxError, ImportError, UnicodeDecodeError,
            UnicodeEncodeError, UnicodeTranslateError, KeyError, BaseExceptionGroup):
    print("declared-str", cls.__str__.__objclass__.__name__)
print("inherited-str", ValueError.__str__.__objclass__.__name__)
print("inherited-repr", KeyError.__repr__.__objclass__.__name__)

error = OSError(2, "missing")
print("os-no-filename", str(error))
error.filename = None
print("os-present-none", str(error))
error.filename2 = None
print("os-present-two-none", str(error))
del error.filename
print("os-deleted-filename", str(error))
error = SyntaxError("bad", ("/tmp/sample.py", 12, 1, "source"))
print("syntax-location", str(error))
error.lineno = True
print("syntax-exact-line", str(error))
error.lineno = 1 << 100
print("syntax-line-overflow", str(error))

# Unicode slots evaluate reason, then encoding, then the current input/positions.
class ChangeEncoding:
    def __str__(self):
        events.append("encoding")
        unicode_error.object = "z"
        unicode_error.start = 0
        unicode_error.end = 1
        return "changed"


class ChangeReason:
    def __str__(self):
        events.append("reason")
        unicode_error.encoding = ChangeEncoding()
        return "why"


unicode_error = UnicodeEncodeError("ascii", "\ud800", 0, 1, "initial")
print("unicode-surrogate", str(unicode_error))
unicode_error.reason = ChangeReason()
events.clear()
print("unicode-callback-order", str(unicode_error), events)
print("unicode-translate", str(UnicodeTranslateError("\ud800", 0, 1, "why")))
print("unicode-decode", str(UnicodeDecodeError("utf-8", b"\xff", 0, 1, "why")))

from urllib.error import ContentTooShortError, HTTPError, URLError

print("url-slots", str(URLError("reason")),
      str(ContentTooShortError("short", b"")), str(HTTPError("u", 404, "missing", {}, None)))


class NamedURLError(ValueError):
    pass


NamedURLError.__name__ = "URLError"
error = NamedURLError("args")
error.reason = "different"
print("url-namesake", str(error))

# Managed type metadata is structural: projection must never call these names
# through a metaclass, and changing qualname must not rename exception repr.
metadata_events = []


class GuardedNames(type):
    def __getattribute__(cls, name):
        if name in ("__name__", "__qualname__", "__module__", "__bases__"):
            metadata_events.append(name)
            raise RuntimeError("metadata callback")
        return super().__getattribute__(name)


class MetadataError(ValueError, metaclass=GuardedNames):
    pass


metadata_error = MetadataError(42)
print("metadata-before", str(metadata_error), repr(metadata_error), metadata_events)
MetadataError.__qualname__ = "Outer.Q"
print("metadata-qualname", str(metadata_error), repr(metadata_error), metadata_events)
MetadataError.__name__ = "prefix.Renamed"
print("metadata-renamed", repr(metadata_error), metadata_events)
# __name__ alone crosses the strict UTF-8 C-name boundary. Rejected
# assignments must preserve the previous stored name and diagnostic identity.
for attempted in ("prefix.\ud800", "prefix.\0tail", "prefix.\0\ud800\udfff"):
    before = type.__getattribute__(MetadataError, "__name__")
    try:
        MetadataError.__name__ = attempted
    except (UnicodeEncodeError, ValueError) as failure:
        details = ((failure.encoding, ascii(failure.object), failure.start,
                    failure.end, failure.reason)
                   if isinstance(failure, UnicodeEncodeError) else str(failure))
        print("metadata-name-rejected", ascii(attempted), type(failure).__name__,
              details, type.__getattribute__(MetadataError, "__name__") == before,
              repr(metadata_error), metadata_events)
    else:
        print("metadata-name-unexpected-success", ascii(attempted))
MetadataError.__name__ = "prefix.é"
print("metadata-name-unicode", ascii(type.__getattribute__(MetadataError, "__name__")),
      ascii(repr(metadata_error)), metadata_events)
for field in ("__qualname__", "__module__"):
    for value in ("\udfff", "nul\0tail", "é"):
        type.__setattr__(MetadataError, field, value)
        print("metadata-python-text", field,
              type.__getattribute__(MetadataError, field) == value,
              ascii(type.__getattribute__(MetadataError, field)),
              ascii(repr(metadata_error)), metadata_events)
for name in ("Type\ud800", "Type\0Name", "Type\0\ud800\udfff", "Typeé"):
    try:
        made = type(name, (), {})
    except (UnicodeEncodeError, ValueError) as failure:
        details = ((failure.encoding, ascii(failure.object), failure.start,
                    failure.end, failure.reason)
                   if isinstance(failure, UnicodeEncodeError) else str(failure))
        print("metadata-create-rejected", ascii(name), type(failure).__name__, details)
    else:
        print("metadata-create-name", ascii(made.__name__), made.__name__ == name)

# Callback strings keep their owner at the root and their code points when
# composed by any Python string producer.
retained = "\ud800"


class RetainedText:
    def __str__(self):
        return retained

    def __repr__(self):
        return retained


retained_payload = RetainedText()
print("owned-roots", str(retained_payload) is retained, repr(retained_payload) is retained,
      object.__str__(retained_payload) is retained, str(ValueError(retained_payload)) is retained)
for label, value in (
    ("exception", ValueError(retained_payload, 1)),
    ("list", [retained_payload]),
    ("tuple", (retained_payload,)),
    ("dict", {1: retained_payload}),
    ("set", {retained_payload}),
    ("frozenset", frozenset({retained_payload})),
    ("slice", slice(retained_payload, None, retained_payload)),
    ("view", {1: retained_payload}.items()),
):
    print("composed", label, ascii(repr(value)))
print("ascii-callback", ascii(retained_payload))
print("percent-callback", ascii("%s/%r/%4.1s" % (retained_payload, retained_payload, retained_payload)))
print("percent-literal", ascii("\udfff%s" % retained_payload))
print("percent-char", ascii("%c/%c" % (retained, 0xD800)))
print("format-root", format(retained_payload) is retained)
print("format-callback", ascii("x{!s:>3}/{!r:.1}".format(retained_payload, retained_payload)))
print("format-literal", ascii("\udfff{}".format(retained_payload)))


class RetainedFormat:
    def __format__(self, spec):
        return retained


print("format-slot", format(RetainedFormat()) is retained, ascii("x{}y".format(RetainedFormat())))
unicode_error = UnicodeEncodeError("ascii", "x", 0, 1, "initial")
unicode_error.reason = retained_payload
unicode_error.encoding = retained_payload
print("unicode-callback-text", ascii(str(unicode_error)))

import pprint
import traceback

print("pprint-callback", ascii(pprint.pformat(retained_payload)))
print("traceback-callback", ascii("".join(traceback.format_exception_only(ValueError(retained_payload)))))
print("traceback-frame-text", ascii("".join(traceback.format_list([
    ("\ud800.py", 3, "\udfff", "  x = '\ud800'"),
]))))


print("format-codepoints", ascii(format("x", "\ud800>3")),
      ascii(format(42, "\ud800>4")), ascii(format(1.5, "\ud800>5")),
      ascii(format(1 + 2j, "\ud800>8")), ascii(format(0xD800, "c")))

for scalar_base, scalar_arg in ((str, "value"), (int, 42), (float, 1.5),
                                (complex, 1 + 2j), (bytes, b"value"),
                                (bytearray, b"value")):
    scalar_child = type("ScalarText", (scalar_base,), {
        "__str__": lambda self: retained,
        "__repr__": lambda self: retained,
    })
    scalar_value = scalar_child(scalar_arg)
    print("scalar-callback", scalar_base.__name__,
          str(scalar_value) is retained, repr(scalar_value) is retained)

class StringWithRepr(str):
    def __repr__(self):
        return retained

string_with_repr = StringWithRepr("contents")
print("declaring-string-slot", str(string_with_repr),
      str.__str__(string_with_repr), type(str(string_with_repr)) is str)

class FailingRender:
    def __repr__(self):
        events.append("first")
        raise LookupError("render stopped")

class LaterRender:
    def __repr__(self):
        events.append("later")
        return "later"

for render in (
    lambda: repr([FailingRender(), LaterRender()]),
    lambda: "%r/%r" % (FailingRender(), LaterRender()),
    lambda: "{!r}/{!r}".format(FailingRender(), LaterRender()),
):
    events.clear()
    try:
        render()
    except LookupError as failure:
        print("render-short-circuit", str(failure), events)

frame_summary = traceback.FrameSummary(
    "\ud800.py", 3, "\udfff", line="  x = '\ud800'",
    colno=2, end_colno=9,
)
try:
    frame_columns = "".join(traceback.format_list([frame_summary]))
except UnicodeEncodeError as failure:
    print("traceback-frame-columns", type(failure).__name__, failure.encoding,
          ascii(failure.object), failure.start, failure.end, failure.reason)
else:
    print("traceback-frame-columns", ascii(frame_columns))

class PprintOverride(list):
    def __repr__(self):
        return retained

print("pprint-container-callback", ascii(pprint.pformat(PprintOverride([1]))))

# format(), brace formatting and f-strings share the same owned callback contract.
format_events = []
format_failure = LookupError("format callback failed")
format_mode = "success"


class FormatText(str):
    pass


format_retained = FormatText("\ud800\udfff\x00")
format_spec = FormatText("\udfff\x00")


class FormatCallable:
    def __call__(self, spec):
        format_events.append(("call", ascii(spec)))
        if format_mode == "raise":
            raise format_failure
        if format_mode == "bad":
            return 7
        return format_retained

    def __del__(self):
        format_events.append("released-callable")


class FormatDescriptor:
    def __get__(self, instance, owner):
        format_events.append("bind")
        return FormatCallable()


class FormatReceiver:
    __format__ = FormatDescriptor()


format_receiver = FormatReceiver()
format_receiver.__format__ = lambda spec: "ignored instance entry"
for format_mode in ("success", "bad", "raise"):
    for entry, render in (
        ("builtin", lambda: format(format_receiver, format_spec)),
        ("brace", lambda: "{:\udfff\x00}".format(format_receiver)),
        ("mapping", lambda: "{value:\udfff\x00}".format_map({"value": format_receiver})),
        ("fstring", lambda: f"{format_receiver:\udfff\x00}"),
    ):
        format_events.clear()
        try:
            format_value = render()
            print("format-contract", format_mode, entry, ascii(format_value),
                  format_value is format_retained, format_events)
        except Exception as failure:
            print("format-contract", format_mode, entry, type(failure).__name__,
                  str(failure), failure is format_failure, format_events)


class FailingFormatDescriptor:
    def __get__(self, instance, owner):
        raise format_failure


class FailingFormatBinding:
    __format__ = FailingFormatDescriptor()

    def __str__(self):
        raise AssertionError("binding failure must not fall through to str")


for render in (
    lambda: format(FailingFormatBinding()),
    lambda: "{}".format(FailingFormatBinding()),
    lambda: "{value}".format_map({"value": FailingFormatBinding()}),
):
    try:
        render()
    except LookupError as failure:
        print("format-binding-failure", failure is format_failure)


class CheckedFormatSpec:
    def __format__(self, spec):
        print("format-spec-identity", spec is format_spec, ascii(spec))
        return format_retained


print("format-result-identity", format(CheckedFormatSpec(), format_spec) is format_retained)

for scalar_base, scalar_arg in ((str, "value"), (int, 42), (float, 1.5),
                                (complex, 1 + 2j), (bytes, b"value"),
                                (bytearray, b"value")):
    scalar_child = type("ScalarFormat", (scalar_base,), {
        "__format__": lambda self, spec: 7,
    })
    try:
        format(scalar_child(scalar_arg))
    except TypeError as failure:
        print("scalar-format-result", scalar_base.__name__, str(failure))

# Invalid owned results are released after the authoritative TypeError exists;
# a destructor's failure stays in the unraisable channel.
import sys

format_unraisable = []
saved_unraisablehook = sys.unraisablehook
sys.unraisablehook = lambda event: format_unraisable.append(type(event.exc_value).__name__)


class InvalidFormatResult:
    def __del__(self):
        format_events.append("released-result")
        raise RuntimeError("format result destructor")


class InvalidOwnedFormat:
    def __format__(self, spec):
        return InvalidFormatResult()


try:
    for render in (lambda: format(InvalidOwnedFormat()), lambda: "{}".format(InvalidOwnedFormat())):
        format_events.clear()
        format_unraisable.clear()
        try:
            render()
        except TypeError as failure:
            print("format-owned-failure", str(failure), format_events, format_unraisable)
finally:
    sys.unraisablehook = saved_unraisablehook

# Percent parser slices are bytes, but diagnostics report Python codepoints.
for format_pattern in (
    "é%q", "😀%q", "\ud800%q", "\ud800\udfff%q", "\x00%q",
    "%é", "%€", "%😀", "%\ud800", "%\udfff", "%\ud800\udfff",
    "%\x00", "%\x1f", "%\x7f", "é%03.2q", "é%(clé)q", "é%%x%q",
):
    format_args = {"clé": 1} if "(" in format_pattern else 1
    try:
        format_pattern % format_args
    except ValueError as failure:
        print("percent-codepoint-error", ascii(format_pattern), ascii(str(failure)))

# The four declaring scalar descriptors share physical formatting primitives.
# Inheritance reaches these descriptors, and explicit base calls bypass an
# overriding __format__. Only the empty spec takes the required str() path.
for scalar_base, scalar_arg, scalar_spec in (
    (str, "é\ud800", ">6s"),
    (int, 42, "08x"),
    (float, 1.5, ".2f"),
    (complex, 1 + 2j, ".2f"),
):
    inherited_child = type("InheritedFormat", (scalar_base,), {})
    override_child = type("OverriddenFormat", (scalar_base,), {
        "__format__": lambda self, spec: 7,
        "__str__": lambda self: "custom-str",
        "__int__": lambda self: (_ for _ in ()).throw(AssertionError("__int__ conversion")),
        "__float__": lambda self: (_ for _ in ()).throw(AssertionError("__float__ conversion")),
    })
    exact_value = scalar_base(scalar_arg)
    inherited_value = inherited_child(scalar_arg)
    override_value = override_child(scalar_arg)
    print("declaring-format", scalar_base.__name__,
          ascii(scalar_base.__format__(exact_value, scalar_spec)),
          ascii(format(inherited_value, scalar_spec)),
          ascii(inherited_value.__format__(scalar_spec)),
          ascii(scalar_base.__format__(override_value, scalar_spec)),
          ascii(scalar_base.__format__(override_value, "")))
    for label, render in (
        ("exact-spec", lambda: scalar_base.__format__(exact_value, 7)),
        ("inherited-spec", lambda: inherited_value.__format__(7)),
        ("base-spec", lambda: scalar_base.__format__(override_value, 7)),
        ("builtin-spec", lambda: format(inherited_value, 7)),
        ("unknown-spec", lambda: format(inherited_value, "q")),
        ("bad-result", lambda: format(override_value, scalar_spec)),
        ("base-receiver", lambda: scalar_base.__format__(object(), scalar_spec)),
    ):
        try:
            render()
        except (TypeError, ValueError) as failure:
            print("declaring-format-error", scalar_base.__name__, label,
                  type(failure).__name__, str(failure))

    bad_str_child = type("BadFormatStr", (scalar_base,), {"__str__": lambda self: 7})
    try:
        scalar_base.__format__(bad_str_child(scalar_arg), "")
    except TypeError as failure:
        print("declaring-empty-format", scalar_base.__name__, str(failure))


# Integer floating presentations invoke the numeric conversion slot even
# through explicit int.__format__; integer presentations read the payload.
numeric_format_events = []


class LargeFormatInt(int):
    def __float__(self):
        numeric_format_events.append("float")
        return 2.5

    def __index__(self):
        raise AssertionError("float slot must precede index")


numeric_renderers = (
    ("format", lambda value: format(value, ".2f")),
    ("base-format", lambda value: int.__format__(value, ".2f")),
    ("brace", lambda value: "{:.2f}".format(value)),
    ("format-map", lambda value: "{x:.2f}".format_map({"x": value})),
    ("f-string", lambda value: f"{value:.2f}"),
    ("percent", lambda value: "%.2f" % value),
)
for magnitude in (42, 10 ** 400):
    for label, render in numeric_renderers:
        numeric_format_events.clear()
        print("integer-float-format", label, magnitude == 42,
              render(LargeFormatInt(magnitude)), numeric_format_events)
numeric_format_events.clear()
print("integer-payload-format", format(LargeFormatInt(42), "04d"),
      int.__float__(LargeFormatInt(42)), numeric_format_events,
      format(True), int.__format__(True, "04d"))


class InheritedFloatInt(int):
    def __index__(self):
        raise AssertionError("inherited int float slot must precede index")


print("inherited-int-float", format(InheritedFloatInt(42), ".2f"),
      InheritedFloatInt.__float__.__objclass__ is int)
for value in (10 ** 400, InheritedFloatInt(10 ** 400)):
    try:
        format(value, ".2f")
    except OverflowError as failure:
        print("integer-float-overflow", type(value).__name__, str(failure))

numeric_format_failure = TypeError("numeric format callback identity")


class RaisingFormatInt(int):
    def __float__(self):
        raise numeric_format_failure

    def __index__(self):
        raise AssertionError("failed float must not try index")


class InvalidFormatInt(int):
    def __float__(self):
        return 7


for cls in (RaisingFormatInt, InvalidFormatInt):
    for label, render in numeric_renderers:
        try:
            render(cls(42))
        except TypeError as failure:
            print("integer-float-error", cls.__name__, label,
                  failure is numeric_format_failure, str(failure))


class BindingFloat:
    def __get__(self, instance, owner):
        raise numeric_format_failure


class BindingFormatInt(int):
    __float__ = BindingFloat()


for label, render in numeric_renderers:
    try:
        render(BindingFormatInt(42))
    except TypeError as failure:
        print("integer-float-binding", label, failure is numeric_format_failure)


class ReturnedFormatFloat(float):
    def __float__(self):
        raise AssertionError("float result admission must read returned payload")


class SubclassResultFormatInt(int):
    def __float__(self):
        return ReturnedFormatFloat(3.25)


import warnings

for label, render in numeric_renderers:
    with warnings.catch_warnings(record=True) as recorded:
        warnings.simplefilter("always", DeprecationWarning)
        print("integer-float-result-subclass", label,
              render(SubclassResultFormatInt(42)),
              [warning.category.__name__ for warning in recorded])
    with warnings.catch_warnings():
        warnings.simplefilter("error", DeprecationWarning)
        try:
            render(SubclassResultFormatInt(42))
        except DeprecationWarning as failure:
            print("integer-float-warning-error", label, str(failure))


class ConvertedFormatFloat(float):
    def __float__(self):
        numeric_format_events.append("float-subclass")
        return 9.5


numeric_format_events.clear()
float_subclass_value = ConvertedFormatFloat(1.25)
print("float-conversion-vs-payload", float(float_subclass_value),
      complex(float_subclass_value), format(float_subclass_value, ".2f"),
      "%.2f" % float_subclass_value, float.__float__(float_subclass_value),
      type(float.__float__(float_subclass_value)) is float, numeric_format_events)


# Notes retain presence separately from Python None, including across C views.
note_error = ValueError("notes")
print("notes-initial", hasattr(note_error, "__notes__"))
for note_value in (None, 0.0, 42, "ab", ("tuple",), ["list"]):
    note_error.__notes__ = note_value
    print("notes-value", note_error.__notes__ is note_value)
    try:
        note_error.add_note("extra")
    except TypeError as failure:
        print("notes-append", type(failure).__name__, str(failure))
    else:
        print("notes-append", note_error.__notes__)
    group = ExceptionGroup("notes", [ValueError("v"), TypeError("t")])
    group.__notes__ = note_value
    matched, rest = group.split(ValueError)
    print("notes-split", getattr(matched, "__notes__", "absent"),
          getattr(rest, "__notes__", "absent"))
    if hasattr(matched, "__notes__"):
        print("notes-independent", matched.__notes__ is not rest.__notes__,
              matched.__notes__ is not note_value)
    del note_error.__notes__
    print("notes-deleted", hasattr(note_error, "__notes__"))
note_error.add_note("after-delete")
print("notes-recreated", note_error.__notes__)


class NoteList(list):
    def append(self, value):
        raise AssertionError("add_note must use list storage")


note_error.__notes__ = NoteList()
note_error.add_note("subclass")
print("notes-list-subclass", note_error.__notes__)


class NoteGetter(ValueError):
    @property
    def __notes__(self):
        raise LookupError("notes getter")


try:
    NoteGetter().add_note("ignored")
except LookupError as failure:
    print("notes-getter-error", str(failure))


class NoteSetter(ValueError):
    @property
    def __notes__(self):
        raise AttributeError("absent")

    @__notes__.setter
    def __notes__(self, value):
        print("notes-setter-before-append", value)
        raise LookupError("notes setter")


try:
    NoteSetter().add_note("ignored")
except LookupError as failure:
    print("notes-setter-error", str(failure))


class NoteSequence:
    def __len__(self):
        return 2

    def __getitem__(self, index):
        if index < 2:
            return "item-" + str(index)
        raise IndexError


class NoteMapping(dict):
    pass


class BrokenNoteSequence(NoteSequence):
    def __iter__(self):
        raise LookupError("notes iterator")


for note_value in (NoteSequence(), NoteMapping(a="ignored"), BrokenNoteSequence()):
    group = ExceptionGroup("notes-protocol", [ValueError("v"), TypeError("t")])
    group.__notes__ = note_value
    try:
        matched, rest = group.split(ValueError)
    except LookupError as failure:
        print("notes-sequence-error", str(failure))
    else:
        print("notes-sequence", getattr(matched, "__notes__", "absent"),
              getattr(rest, "__notes__", "absent"))


class ExceptionsList(list):
    def __iter__(self):
        return iter([TypeError("from override")])


group = ExceptionGroup("constructor-sequence", ExceptionsList([ValueError("storage")]))
print("group-sequence-subclass", type(group.exceptions[0]).__name__)
try:
    ExceptionGroup("mapping", NoteMapping(a=ValueError("v")))
except TypeError as failure:
    print("group-mapping-rejected", str(failure))


class NotesHooks(ValueError):
    def __getattribute__(self, name):
        if name == "__notes__":
            print("notes-hook-get")
        return super().__getattribute__(name)

    def __getattr__(self, name):
        if name == "__notes__":
            print("notes-hook-missing")
        raise AttributeError(name)

    def __setattr__(self, name, value):
        if name == "__notes__":
            print("notes-hook-set", value)
        return super().__setattr__(name, value)

    def __delattr__(self, name):
        print("notes-hook-delete", name)
        return super().__delattr__(name)


note_hooks = NotesHooks()
note_hooks.add_note("first")
print("notes-hook-dict", note_hooks.__dict__["__notes__"])
note_hooks.__dict__["__notes__"] = ["direct"]
print("notes-hook-direct", note_hooks.__notes__)
del note_hooks.__dict__["__notes__"]
print("notes-hook-direct-delete", hasattr(note_hooks, "__notes__"))
object.__setattr__(note_hooks, "__notes__", ["object default"])
object.__delattr__(note_hooks, "__notes__")
print("notes-object-default", "__notes__" in note_hooks.__dict__)


class NotesProperty(ValueError):
    @property
    def __notes__(self):
        return self.saved

    @__notes__.setter
    def __notes__(self, value):
        self.saved = value

    @__notes__.deleter
    def __notes__(self):
        print("notes-property-delete", self.saved)
        del self.saved


note_property = NotesProperty()
object.__setattr__(note_property, "__notes__", ["descriptor"])
object.__delattr__(note_property, "__notes__")
print("notes-property-deleted", hasattr(note_property, "saved"))


class ExceptionFieldMixin:
    args = "mixin-args"
    name = "mixin-name"


class EarlyException(ValueError, ExceptionFieldMixin):
    pass


class EarlyMixin(ExceptionFieldMixin, ValueError):
    pass


class EarlyNameError(NameError, ExceptionFieldMixin):
    pass


class EarlyNameMixin(ExceptionFieldMixin, NameError):
    pass


for field_error in (EarlyException("args"), EarlyMixin("args")):
    print("exception-owner-mro", field_error.args)
    field_error.__dict__["args"] = "instance-args"
    print("exception-owner-dict", field_error.args)
for field_error in (EarlyNameError("name", name="typed"),
                    EarlyNameMixin("name", name="typed")):
    print("exception-typed-owner-mro", field_error.name)
    field_error.__dict__["name"] = "instance-name"
    print("exception-typed-owner-dict", field_error.name)


import operator


class HintDescriptor:
    def __get__(self, instance, owner):
        raise TypeError("hint descriptor failure")


class LookupHintFailure:
    __length_hint__ = HintDescriptor()


class CallHintFailure:
    def __length_hint__(self):
        raise TypeError("hint call failure")


class HintProtocol:
    def __init__(self, value):
        self.value = value

    def __getattribute__(self, name):
        if name == "__length_hint__":
            raise AssertionError("hint lookup used ordinary attribute access")
        return super().__getattribute__(name)

    def __length_hint__(self):
        return self.value


for hint_source in (LookupHintFailure(), CallHintFailure(), HintProtocol(3),
                    HintProtocol(NotImplemented), HintProtocol(-1), HintProtocol(1.5)):
    try:
        print("length-hint", operator.length_hint(hint_source, 9))
    except (TypeError, ValueError) as failure:
        print("length-hint-error", type(failure).__name__, str(failure))


from types import MappingProxyType

for mapping_only in (MappingProxyType({"v": ValueError("v")}), list[int]):
    try:
        ExceptionGroup("mapping-only", mapping_only)
    except TypeError as failure:
        print("group-mapping-only", str(failure))
print("notes-dictionary-only", "__notes__" not in BaseException.__dict__)


class SpoofNotesList:
    @property
    def __class__(self):
        return list


spoof_notes = ValueError("spoof")
spoof_notes.__notes__ = SpoofNotesList()
try:
    spoof_notes.add_note("rejected")
except TypeError as failure:
    # The diagnostic includes the ordinary object's address on CPython.
    print("notes-spoof-rejected", type(failure).__name__)

for invalid_note in (None, 17, b"bytes"):
    try:
        ValueError().add_note(invalid_note)
    except TypeError as failure:
        print("notes-invalid-value", str(failure))

blocking = BlockingIOError()
for attempt in range(2):
    try:
        del blocking.characters_written
    except AttributeError as failure:
        print("characters-unset-delete", type(failure).__name__)
    else:
        print("characters-unset-delete", "ok")


class IndexOnly:
    def __index__(self):
        print("unicode-index-called")
        return 1


unicode_error = UnicodeDecodeError("ascii", b"x", 0, 1, "bad")
for field in ("start", "end"):
    for value in (IndexOnly(), 1.5, True, 2):
        try:
            setattr(unicode_error, field, value)
        except TypeError as failure:
            print("unicode-offset-rejected", field, type(failure).__name__)
        else:
            print("unicode-offset", field, getattr(unicode_error, field))


class PlainGroup(ExceptionGroup):
    pass


group_items = (ValueError("v"), TypeError("t"))
plain_group = PlainGroup("plain", group_items)
print("group-tuple-identity", plain_group.exceptions is group_items)
print("group-default-derive", type(plain_group.derive(group_items)).__name__)
print("group-default-split", *(type(part).__name__ for part in plain_group.split(ValueError)))


class DerivedGroup(ExceptionGroup):
    def derive(self, items):
        print("group-derive-called", len(items))
        return DerivedGroup(self.message, items)


def select_value(error):
    print("group-predicate", type(error).__name__)
    return isinstance(error, ValueError)


for subgroup_only in (False, True):
    print("group-mode", subgroup_only)
    derived = DerivedGroup("custom", group_items)
    derived.add_note("copy")
    if subgroup_only:
        matched = derived.subgroup(select_value)
        parts = (matched,)
    else:
        parts = derived.split(select_value)
    for part in parts:
        print("group-derived-part", type(part).__name__, part.__notes__,
              part.__notes__ is derived.__notes__, part.__suppress_context__)


class InvalidDerivedGroup(ExceptionGroup):
    def derive(self, items):
        return 3


try:
    InvalidDerivedGroup("invalid", group_items).subgroup(ValueError)
except TypeError as failure:
    print("group-invalid-derived", str(failure))
