"""Purpose: differential coverage for csv writer lineterminator."""

import csv
import io


buf = io.StringIO()
writer = csv.writer(buf, lineterminator="\r\n")
writer.writerow(["a", "b"])
writer.writerow(["c", "d"])

print(buf.getvalue())
