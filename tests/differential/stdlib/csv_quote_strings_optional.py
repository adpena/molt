"""Purpose: differential coverage for csv quote strings optional."""

import csv
import io


if not hasattr(csv, "QUOTE_STRINGS"):
    print("no_quote_strings")
else:
    buf = io.StringIO()
    writer = csv.writer(buf, quoting=csv.QUOTE_STRINGS, lineterminator="\n")
    writer.writerow(["a", 1, "b"])
    buf.seek(0)
    reader = csv.reader(buf, quoting=csv.QUOTE_STRINGS)
    print(list(reader))
