"""Purpose: differential coverage for csv unicode roundtrip."""

import csv
import io


rows = [["snowman", "\u2603", "\u6f22\u5b57", "\U0001f680"]]
buf = io.StringIO()
writer = csv.writer(buf, lineterminator="\n")
writer.writerows(rows)

buf.seek(0)
reader = csv.reader(buf)
print(list(reader))
