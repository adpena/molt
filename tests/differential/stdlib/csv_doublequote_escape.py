"""Purpose: differential coverage for csv doublequote escape."""

import csv
import io


buf = io.StringIO()
writer = csv.writer(
    buf,
    delimiter=",",
    quotechar='"',
    doublequote=False,
    escapechar="\\",
    lineterminator="\n",
)
writer.writerow(['a"b', "c"])

buf.seek(0)
reader = csv.reader(
    buf,
    delimiter=",",
    quotechar='"',
    doublequote=False,
    escapechar="\\",
)
print(list(reader))
