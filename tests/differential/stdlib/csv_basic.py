"""Purpose: differential coverage for csv basic."""

import csv
import io


buf = io.StringIO()
writer = csv.writer(buf)
writer.writerow(["a", "b", 1])
writer.writerow(["x,y", "z"])

buf.seek(0)
reader = csv.reader(buf)
rows = list(reader)
print(rows)
