"""Purpose: differential coverage for csv extrasaction raise."""

import csv
import io


buf = io.StringIO()
writer = csv.DictWriter(buf, fieldnames=["a"], extrasaction="raise")
try:
    writer.writerow({"a": 1, "b": 2})
except Exception as exc:
    print(type(exc).__name__)
