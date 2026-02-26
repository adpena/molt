"""Purpose: differential coverage for csv.writer object stringification."""

import csv
import io


class Cell:
    def __str__(self) -> str:
        return "cell-value"


buf = io.StringIO()
writer = csv.writer(buf, lineterminator="\n")
writer.writerow([Cell(), 1])
print(buf.getvalue().strip())
