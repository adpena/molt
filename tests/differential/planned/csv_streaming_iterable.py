"""Purpose: differential coverage for csv streaming iterable."""

import csv


lines = ["a,b\n", "1,2\n", "3,4\n"]
reader = csv.reader(iter(lines))
print(list(reader))
