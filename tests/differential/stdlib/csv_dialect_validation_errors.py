"""Purpose: differential coverage for csv dialect/writer validation errors."""

import csv
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
