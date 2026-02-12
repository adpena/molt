"""Purpose: differential coverage for csv sniffer dialects more."""

import csv
import io


data = "a|b|c\n1|2|3\n"
dialect = csv.Sniffer().sniff(data)
print(dialect.delimiter)

buf = io.StringIO(data)
reader = csv.reader(buf, dialect)
print(list(reader))
