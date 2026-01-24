"""Purpose: differential coverage for csv register dialect."""

import csv
import io


csv.register_dialect("pipe", delimiter="|", lineterminator="\n")

buf = io.StringIO()
writer = csv.writer(buf, dialect="pipe")
writer.writerow(["a", "b", "c"])

buf.seek(0)
reader = csv.reader(buf, dialect="pipe")
print(list(reader))

csv.unregister_dialect("pipe")
