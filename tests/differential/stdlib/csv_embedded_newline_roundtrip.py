"""Purpose: differential coverage for csv reader records that span multiple
physical lines via a quoted field with an embedded newline.

Regression for the reader signaling contract: an unterminated quoted field
(an open ``"`` with no closing ``"`` before the current physical line ends)
must report "needs more data" so ``_Reader.__next__`` appends the next physical
line and continues parsing the quoted field — rather than returning a partial
row that tears the field across rows. At true EOF, a non-strict dialect commits
the truncated field while a strict dialect raises "unexpected end of data".
This mirrors CPython's ``_csv`` state machine exactly.
"""

import csv
import io


# Writer is correct; round-trip must reconstruct the embedded newline in one row.
rows = [['a"b', "c"], ["line\nbreak", "x"]]
buf = io.StringIO()
csv.writer(buf, quoting=csv.QUOTE_MINIMAL, lineterminator="\n").writerows(rows)
print("written:", repr(buf.getvalue()))
buf.seek(0)
print("roundtrip:", list(csv.reader(buf)))

# A quoted field whose embedded newline closes mid-stream, then more data.
print("multiline:", list(csv.reader(io.StringIO('"line\nbreak",x\n'))))

# Multiple embedded newlines inside one field.
print("multi_nl:", list(csv.reader(io.StringIO('"a\nb\nc",2\n'))))

# Unterminated quote at true EOF (no closing quote ever) — non-strict commits.
print("eof_unterminated:", list(csv.reader(io.StringIO('"unterminated'))))
print("eof_unterminated_nl:", list(csv.reader(io.StringIO('"unterminated\nmore'))))

# A normal multi-row file still parses row-per-line.
print("plain:", list(csv.reader(io.StringIO("a,b\n1,2\n"))))

# strict=True: an unterminated quote at EOF raises "unexpected end of data".
try:
    list(csv.reader(io.StringIO('"unterminated'), strict=True))
    print("strict_eof: NO ERROR")
except csv.Error as exc:
    print("strict_eof:", str(exc))

# strict=True with a complete multi-line quoted record is fine.
print("strict_ok:", list(csv.reader(io.StringIO('"a\nb",c\n'), strict=True)))
