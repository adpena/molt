"""Purpose: differential coverage for csv strict error."""

import csv
import io


data = '"a","b\n'

buf = io.StringIO(data)
reader = csv.reader(buf, strict=True)
try:
    list(reader)
except Exception as exc:
    print(type(exc).__name__)
