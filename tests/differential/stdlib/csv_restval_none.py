"""Purpose: differential coverage for csv restval none."""

import csv
import io


data = "a,b\n1\n"
buf = io.StringIO(data)
reader = csv.DictReader(buf, restval=None)
print(list(reader))
