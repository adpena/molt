"""Purpose: differential coverage for csv quoting modes."""

import csv
import io


rows = [["a", "b"], ["1", "2"]]

buf = io.StringIO()
writer = csv.writer(buf, quoting=csv.QUOTE_ALL, lineterminator="\n")
writer.writerows(rows)

buf.seek(0)
print(buf.getvalue().strip())
