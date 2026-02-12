"""Purpose: differential coverage for csv dictwriter restval."""

import csv
import io


buf = io.StringIO()
writer = csv.DictWriter(buf, fieldnames=["a", "b"], restval="X")
writer.writeheader()
writer.writerow({"a": 1})

buf.seek(0)
reader = csv.DictReader(buf)
print(list(reader))
