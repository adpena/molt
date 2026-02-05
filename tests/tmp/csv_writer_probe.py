import csv
import io

buf = io.StringIO()
writer = csv.writer(buf)
writer.writerow(["a", "b", 1])
print(buf.getvalue())
