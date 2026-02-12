"""Purpose: differential coverage for csv field size limit error."""

import csv
import io


data = "a" * 10 + "\n"
buf = io.StringIO(data)
old = csv.field_size_limit(5)
try:
    reader = csv.reader(buf)
    list(reader)
except Exception as exc:
    print(type(exc).__name__)
finally:
    csv.field_size_limit(old)
