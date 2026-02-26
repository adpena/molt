"""Purpose: differential coverage for top-level _csv module roundtrip."""

import _csv
import io

buf = io.StringIO()
writer = _csv.writer(buf)
writer.writerow(["a", 1, None])
writer.writerow(["x,y", "z"])

buf.seek(0)
reader = _csv.reader(buf)
print(list(reader))
