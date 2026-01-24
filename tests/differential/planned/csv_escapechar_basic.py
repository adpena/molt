"""Purpose: differential coverage for csv escapechar basic."""

import csv
import io


buf = io.StringIO()
writer = csv.writer(buf, escapechar="\\", quoting=csv.QUOTE_NONE, lineterminator="\n")
writer.writerow(["a,b", "c\\d", "e"])

buf.seek(0)
reader = csv.reader(buf, escapechar="\\", quoting=csv.QUOTE_NONE)
print(list(reader))
