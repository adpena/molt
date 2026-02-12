"""Purpose: differential coverage for csv newline quoting."""

import csv
import io


buf = io.StringIO(newline="")
writer = csv.writer(buf, lineterminator="\n")
writer.writerow(["a", "b\nline", "c"])
writer.writerow(["1", "2", "3"])

buf.seek(0)
reader = csv.reader(buf)
print(list(reader))
