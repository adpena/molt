"""Purpose: csv.writer shares the canonical CPython float repr authority."""

import csv
import io


buf = io.StringIO()
csv.writer(buf, lineterminator="\n").writerow(
    [1e16, 1e-5, 1e100, 5e-324, 137839762462415.62, -0.0]
)
print(buf.getvalue().strip())
