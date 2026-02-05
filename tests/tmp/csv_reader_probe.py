import csv
import io

buf = io.StringIO("a,b,1\nx,y,2\n")
reader = csv.reader(buf)
print(list(reader))
