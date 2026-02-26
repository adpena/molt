"""Purpose: verify csv dialect kw + register_dialect override semantics."""

import csv
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
