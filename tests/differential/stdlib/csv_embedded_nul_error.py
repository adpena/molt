"""Purpose: differential coverage for csv embedded nul error."""

import csv
import io


value = bytes([0x61, 0x00, 0x62]).decode("latin-1")
data = f'"{value}",x\n'

buf = io.StringIO(data)
reader = csv.reader(buf)
try:
    list(reader)
except Exception as exc:
    print(type(exc).__name__)
