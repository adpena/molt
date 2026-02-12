"""Purpose: differential coverage for csv sniffer basic."""

import csv
import io


data = "a,b,c\n1,2,3\n"
sniffer = csv.Sniffer()
dialect = sniffer.sniff(data)
print(dialect.delimiter)

buf = io.StringIO(data)
reader = csv.reader(buf, dialect)
print(list(reader))
