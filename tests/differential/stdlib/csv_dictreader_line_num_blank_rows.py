"""Purpose: differential coverage for csv.DictReader line_num/blank-row parity."""

import csv
import io


def main():
    data = "a,b\n\n1,2,3\n\n4,5\n"
    reader = csv.DictReader(io.StringIO(data))
    print("line_num_start", reader.line_num)
    rows = []
    for row in reader:
        rows.append(row)
        print("line_num", reader.line_num, row)
    print("rows", rows)

    fields_iter = iter(["x", "y"])
    reader2 = csv.DictReader(io.StringIO("7,8\n"), fieldnames=fields_iter)
    print("row2", next(reader2))
    print("fieldnames2", list(reader2.fieldnames))


if __name__ == "__main__":
    main()
