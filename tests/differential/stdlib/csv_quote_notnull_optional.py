"""Purpose: differential coverage for csv quote notnull optional."""

import csv
import io


if not hasattr(csv, "QUOTE_NOTNULL"):
    print("no_quote_notnull")
else:
    buf = io.StringIO()
    writer = csv.writer(buf, quoting=csv.QUOTE_NOTNULL, lineterminator="\n")
    writer.writerow([None, "x", "y"])
    buf.seek(0)
    reader = csv.reader(buf, quoting=csv.QUOTE_NOTNULL)
    print(list(reader))
