# MOLT_META: stderr=exact
"""Purpose: differential coverage for csv basic."""

import csv
import io
import _csv


buf = io.StringIO()
writer = csv.writer(buf)
writer.writerow(["a", "b", 1])
writer.writerow(["x,y", "z"])

buf.seek(0)
reader = csv.reader(buf)
rows = list(reader)
print(rows)


# The caller's row and every field must remain alive after normalization
# returns the same object. Repeated writes and final retirement distinguish an
# owned identity return from consuming the caller's reference.
retired = []


class Field:
    def __init__(self, label):
        self.label = label

    def __str__(self):
        return self.label

    def __del__(self):
        retired.append(self.label)


class ListRow(list):
    pass


class TupleRow(tuple):
    pass


for row_type in (list, tuple, ListRow, TupleRow):
    label = row_type.__name__
    row = row_type([Field(label)])
    output = io.StringIO()
    sink = csv.writer(output)
    sink.writerow(row)
    sink.writerows([row, row])
    print("retained", label, row[0].label, retired, repr(output.getvalue()))
    del row
    print("retired", label, retired)


# CSV owns its original exception class even after public aliases are rebound.

original_error = csv.Error
original_native_error = _csv.Error
old_reader = csv.reader(['"unterminated'], strict=True)
old_writer = csv.writer(io.StringIO(), quoting=csv.QUOTE_NONE)


class ReplacementError(Exception):
    pass


csv.Error = ReplacementError
_csv.Error = ReplacementError
try:
    for label, action in (
        ("old reader", lambda: next(old_reader)),
        ("old writer", lambda: old_writer.writerow(["a,b"])),
        ("new reader", lambda: next(csv.reader(['"unterminated'], strict=True))),
        (
            "new writer",
            lambda: csv.writer(io.StringIO(), quoting=csv.QUOTE_NONE).writerow(["a,b"]),
        ),
        ("unknown dialect", lambda: csv.get_dialect("molt_missing_provenance")),
        ("unregister", lambda: csv.unregister_dialect("molt_missing_provenance")),
        ("non-string dialect", lambda: csv.get_dialect(23)),
    ):
        try:
            action()
            print(label, "accepted")
        except Exception as exc:
            print(label, type(exc) is original_error)
finally:
    csv.Error = original_error
    _csv.Error = original_native_error

# Incomplete input is parser state, not an exception message to interpret.
print("multiline", list(csv.reader(['"first\n', 'second",end\n'], strict=True)))
print("permissive EOF", list(csv.reader(['"first'], strict=False)))

sentinel = ValueError("unexpected end of data")
events = []


class BadText:
    def __str__(self):
        events.append("bad")
        raise sentinel


class LaterText:
    def __str__(self):
        events.append("later")
        return "later"


for many in (False, True):
    output = io.StringIO()
    writer = csv.writer(output)
    events.clear()
    try:
        if many:
            writer.writerows([[BadText(), LaterText()]])
        else:
            writer.writerow([BadText(), LaterText()])
        print("callback", many, "accepted")
    except ValueError as exc:
        print("callback", many, exc is sentinel, events, output.getvalue() == "")

try:
    next(csv.reader(["not-a-number\n"], quoting=csv.QUOTE_NONNUMERIC))
    print("numeric reader", "accepted")
except ValueError as exc:
    print("numeric reader", type(exc) is ValueError)


# Only iterator creation translates TypeError; advancing an admitted iterator
# preserves its exception, and must not enter __iter__ a second time.
iteration_error = TypeError("row iteration callback")


class ProtocolRow:
    def __init__(self, failure):
        self.failure = failure
        self.iter_calls = 0
        self.next_calls = 0

    def __iter__(self):
        self.iter_calls += 1
        if self.iter_calls != 1:
            raise AssertionError("row protocol entered twice")
        if self.failure == "iter":
            raise iteration_error
        return self

    def __next__(self):
        self.next_calls += 1
        if self.failure == "next":
            raise iteration_error
        if self.next_calls == 1:
            return "field"
        raise StopIteration


for many in (False, True):
    for failure in ("iter", "next", "none"):
        row = ProtocolRow(failure)
        output = io.StringIO()
        writer = csv.writer(output)
        try:
            if many:
                writer.writerows([row])
            else:
                writer.writerow(row)
            result = ("written", output.getvalue() == "field\r\n")
        except Exception as exc:
            result = (
                "error",
                type(exc) is original_error,
                exc is iteration_error,
                output.getvalue() == "",
            )
        print("row protocol", many, failure, row.iter_calls, row.next_calls, result)
