"""Purpose: differential coverage for csv excel tab."""

import csv
import io


buf = io.StringIO()
writer = csv.writer(buf, dialect=csv.excel_tab, lineterminator="\n")
writer.writerow(["a", "b", "c"])

buf.seek(0)
reader = csv.reader(buf, dialect=csv.excel_tab)
print(list(reader))
