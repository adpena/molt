"""Purpose: differential coverage for csv none fields."""

import csv
import io


buf = io.StringIO()
writer = csv.writer(buf, lineterminator="\n")
writer.writerow([None, "", "x"])

buf.seek(0)
reader = csv.reader(buf)
print(list(reader))
