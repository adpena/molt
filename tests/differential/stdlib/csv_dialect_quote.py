"""Purpose: differential coverage for csv dialect quote."""

import csv
import io


buf = io.StringIO()
writer = csv.writer(buf, delimiter=";", quotechar="'", lineterminator="\n")
writer.writerow(["a;1", "b'2", "c"])

buf.seek(0)
reader = csv.reader(buf, delimiter=";", quotechar="'")
print(list(reader))
