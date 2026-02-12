"""Purpose: differential coverage for csv malformed unterminated quote."""

import csv
import io


data = '"a","b\n1,2\n'
buf = io.StringIO(data)
reader = csv.reader(buf)
try:
    list(reader)
except Exception as exc:
    print(type(exc).__name__)
