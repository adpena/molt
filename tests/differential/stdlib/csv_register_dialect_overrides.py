"""Purpose: verify csv dialect kw + register_dialect override semantics."""

import csv
import _csv
import io


csv.register_dialect("pipe_kw", dialect="excel", delimiter="|")
csv.register_dialect("pipe_obj", csv.excel, delimiter=":")

buf = io.StringIO()
writer = csv.writer(buf, dialect="pipe_kw")
writer.writerow(["a", "b"])

buf.seek(0)
print("registered_kw", next(csv.reader(buf, dialect="pipe_kw")))

buf = io.StringIO()
writer = csv.writer(buf, dialect="pipe_obj")
writer.writerow(["c", "d"])

buf.seek(0)
print("registered_obj", next(csv.reader(buf, dialect="pipe_obj")))

buf = io.StringIO()
writer = csv.writer(buf, dialect="excel", delimiter=";")
writer.writerow(["x", "y"])

buf.seek(0)
print("keyword", next(csv.reader(buf, dialect="excel", delimiter=";")))

try:
    csv.writer(io.StringIO(), "excel", dialect="unix")
except Exception as exc:  # noqa: BLE001
    print("duplicate", type(exc).__name__)

try:
    csv.register_dialect("bad_fmtparam", dialect="excel", bogus=1)
except Exception as exc:  # noqa: BLE001
    print("register_unknown", type(exc).__name__)

try:
    csv.writer(io.StringIO(), dialect="excel", bogus=1)
except Exception as exc:  # noqa: BLE001
    print("writer_unknown", type(exc).__name__)

csv.unregister_dialect("pipe_kw")
csv.unregister_dialect("pipe_obj")


class Pipe(csv.excel):
    delimiter = "|"
    lineterminator = "\n"


class InheritedPipe(Pipe):
    skipinitialspace = True


configured = InheritedPipe()
configured.delimiter = ":"
for label, dialect, line in [
    ("class", InheritedPipe, "a| b"),
    ("instance", configured, "a: b"),
    ("excel class", csv.excel, "a,b"),
    ("excel instance", csv.excel(), "a,b"),
]:
    reader = csv.reader([line], dialect)
    writer = csv.writer(io.StringIO(), dialect)
    print(
        "dialect source",
        label,
        list(reader),
        reader.dialect.delimiter,
        writer.dialect.delimiter,
        isinstance(reader.dialect, _csv.Dialect),
    )

normalized = _csv.Dialect(configured)
configured.delimiter = ";"
print("dialect snapshot", normalized.delimiter, configured.delimiter)
print("dialect reuse", _csv.Dialect(normalized) is normalized)
csv.register_dialect("inherited", InheritedPipe)
registered = csv.get_dialect("inherited")
print("registered class", registered.delimiter, registered.skipinitialspace)
csv.unregister_dialect("inherited")

sniffed = csv.Sniffer().sniff("a;b\nc;d\n")
print("sniffed class", isinstance(sniffed, type), issubclass(sniffed, csv.Dialect))
print("sniffed consumer", list(csv.reader(["a;b"], sniffed)), sniffed().delimiter)
