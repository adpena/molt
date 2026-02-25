"""Purpose: differential coverage for csv.Sniffer.has_header heuristics."""

import csv


def main():
    sniffer = csv.Sniffer()
    samples = [
        ("comma_header", "name,age\nAda,36\n"),
        ("comma_numeric", "1,2\n3,4\n"),
        ("pipe_header", "name|age\nAda|36\n"),
        ("quoted_header", '"name","age"\n"Ada",36\n'),
        ("all_alpha_rows", "A,B\nC,D\n"),
    ]
    for label, sample in samples:
        print(label, sniffer.has_header(sample))


if __name__ == "__main__":
    main()
