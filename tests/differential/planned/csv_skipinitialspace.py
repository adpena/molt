"""Purpose: differential coverage for csv skipinitialspace."""

import csv
import io


data = "a, b,  c\n1, 2,   3\n"
buf = io.StringIO(data)
reader = csv.reader(buf, skipinitialspace=True)
print(list(reader))
