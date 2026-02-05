# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
import os

ROOT = os.path.join("logs", "io_text_encodings")
os.makedirs(ROOT, exist_ok=True)
PATH = os.path.join(ROOT, "seek_cookie_utf8.txt")
PATH_SIG = os.path.join(ROOT, "seek_cookie_utf8_sig.txt")

DATA = "alpha\nbeta\r\ngamma\romega\nEND"

with open(PATH, "wb") as handle:
    handle.write(DATA.encode("utf-8"))

with open(PATH_SIG, "w", encoding="utf-8-sig", newline="") as handle:
    handle.write(DATA)


def exercise(label, path, encoding, newline):
    with open(path, "r", encoding=encoding, newline=newline) as handle:
        first = handle.read(5)
        pos = handle.tell()
        mid = handle.read(4)
        handle.seek(pos)
        mid_again = handle.read(4)
        handle.seek(0)
        line1 = handle.readline()
        pos2 = handle.tell()
        line2 = handle.readline()
        handle.seek(pos2)
        line2_again = handle.readline()
        handle.seek(0)
        chunk = handle.read(12)
        pos3 = handle.tell()
        handle.seek(pos3)
        chunk2 = handle.read(5)
    print("case", label, "newline", newline)
    print("read5", repr(first))
    print("read4", repr(mid), "again4", repr(mid_again))
    print("line1", repr(line1))
    print("line2", repr(line2), "line2b", repr(line2_again))
    print("chunk", repr(chunk), "chunk2", repr(chunk2))


for newline in (None, ""):
    exercise("utf-8", PATH, "utf-8", newline)

for newline in (None, ""):
    exercise("utf-8-sig", PATH_SIG, "utf-8-sig", newline)
