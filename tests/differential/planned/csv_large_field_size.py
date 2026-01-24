"""Purpose: differential coverage for csv large field size."""

import csv
import io


payload = "a" * 1024
buf = io.StringIO(payload + "\n")
reader = csv.reader(buf)
print(len(next(reader)[0]))
