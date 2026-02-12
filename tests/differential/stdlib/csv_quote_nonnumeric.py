"""Purpose: differential coverage for csv quote nonnumeric."""

import csv
import io


buf = io.StringIO()
writer = csv.writer(buf, quoting=csv.QUOTE_NONNUMERIC, lineterminator="\n")
writer.writerow(["a", 1, 2.5])

buf.seek(0)
reader = csv.reader(buf, quoting=csv.QUOTE_NONNUMERIC)
row = next(reader)
print([type(val).__name__ for val in row], row)
