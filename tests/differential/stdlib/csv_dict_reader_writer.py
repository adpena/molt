"""Purpose: differential coverage for csv dict reader writer."""

import csv
import io


buf = io.StringIO()
fieldnames = ["name", "age"]
writer = csv.DictWriter(buf, fieldnames=fieldnames)
writer.writeheader()
writer.writerow({"name": "Ada", "age": 36})
writer.writerow({"name": "Bob", "age": 40})

buf.seek(0)
reader = csv.DictReader(buf)
rows = list(reader)
print(rows)
