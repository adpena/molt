"""Purpose: differential coverage for csv dialect/writer validation errors."""

import csv
import _csv
import io


def show(label, thunk):
    try:
        thunk()
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))


show("delimiter_none", lambda: csv.reader(io.StringIO("a,b"), delimiter=None))
show("quotechar_long", lambda: csv.reader(io.StringIO("a,b"), quotechar="xx"))
show("escapechar_long", lambda: csv.reader(io.StringIO("a,b"), escapechar="xx"))
show("quoting_bad", lambda: csv.reader(io.StringIO("a,b"), quoting=99))
show(
    "quotechar_none_quoted",
    lambda: csv.reader(io.StringIO("a,b"), quotechar=None, quoting=csv.QUOTE_MINIMAL),
)
show("lineterminator_bad", lambda: csv.writer(io.StringIO(), lineterminator=1))
show("register_name_type", lambda: csv.register_dialect(1))
show("get_unknown", lambda: csv.get_dialect("molt_missing_dialect"))
show("unregister_unknown", lambda: csv.unregister_dialect("molt_missing_dialect"))

buf = io.StringIO()
writer = csv.writer(buf)
show("writerow_non_iterable", lambda: writer.writerow(1))

show("base_configuration", csv.Dialect)


class Incomplete(csv.Dialect):
    delimiter = "|"


show("incomplete_configuration", Incomplete)
show("quoting_bool", lambda: _csv.Dialect(quoting=True))
show("quoting_bool_false", lambda: _csv.Dialect(quoting=False))


class IntSubclass(int):
    pass


index_calls = []


class IndexOnly:
    def __index__(self):
        index_calls.append("index")
        return csv.QUOTE_MINIMAL


for label, value in (
    ("quoting_int_subclass", IntSubclass(0)),
    ("quoting_index", IndexOnly()),
):
    try:
        _csv.Dialect(quoting=value)
    except TypeError:
        print(label, "TypeError")
    else:
        print(label, "accepted")
print("quoting index calls", index_calls)

show("native_lineterminator_none", lambda: _csv.Dialect(lineterminator=None))
show("native_conflicting_chars", lambda: _csv.Dialect(delimiter='"'))
show("native_escape_space", lambda: _csv.Dialect(escapechar=" ", skipinitialspace=True))

native = _csv.Dialect(None, "|", 1, None, "\n", None, csv.QUOTE_NONE, 1, 1)
print(
    "native constructor",
    native.delimiter,
    native.doublequote,
    native.quotechar,
    native.skipinitialspace,
    native.strict,
)
print("native omitted quoting", _csv.Dialect(quotechar=None).quoting)
print("reader omitted quoting", csv.reader(["a,b"], quotechar=None).dialect.quoting)
print(
    "writer omitted quoting", csv.writer(io.StringIO(), quotechar=None).dialect.quoting
)
show(
    "native explicit quoting",
    lambda: _csv.Dialect(quotechar=None, quoting=csv.QUOTE_MINIMAL),
)
show("readonly delimiter", lambda: setattr(native, "delimiter", ";"))
show("readonly strict", lambda: setattr(native, "strict", False))
show("readonly deletion", lambda: delattr(native, "quotechar"))

failure = ValueError("dialect conversion sentinel")


class BrokenTruth:
    def __bool__(self):
        raise failure


try:
    _csv.Dialect(doublequote=BrokenTruth(), quotechar="too long")
except ValueError as exc:
    print("conversion order", exc is failure)
