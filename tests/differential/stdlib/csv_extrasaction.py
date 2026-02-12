"""Purpose: differential coverage for csv extrasaction."""

import csv
import io


buf = io.StringIO()
fieldnames = ["name", "age"]
writer = csv.DictWriter(buf, fieldnames=fieldnames, extrasaction="ignore")
writer.writeheader()
writer.writerow({"name": "Ada", "age": 36, "extra": "skip"})

buf.seek(0)
reader = csv.DictReader(buf)
print(list(reader))
