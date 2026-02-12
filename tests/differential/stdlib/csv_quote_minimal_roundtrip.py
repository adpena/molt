"""Purpose: differential coverage for csv quote minimal roundtrip."""

import csv
import io


rows = [["a", "b"], ["x,y", "z"]]

buf = io.StringIO()
writer = csv.writer(buf, quoting=csv.QUOTE_MINIMAL, lineterminator="\n")
writer.writerows(rows)

buf.seek(0)
reader = csv.reader(buf)
print(list(reader))
