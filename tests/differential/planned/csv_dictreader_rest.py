"""Purpose: differential coverage for csv dictreader rest."""

import csv
import io


data = "a,b\n1,2,3,4\n"
buf = io.StringIO(data)
reader = csv.DictReader(buf, restkey="extra", restval="missing")
print(list(reader))
