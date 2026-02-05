# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
import os

ROOT = os.path.join("logs", "io_text_encodings")
os.makedirs(ROOT, exist_ok=True)
PATH = os.path.join(ROOT, "seek_cookie_utf8_multibyte.txt")

DATA = "a😊b\r\nc😊d\n"

with open(PATH, "w", encoding="utf-8", newline="") as handle:
    handle.write(DATA)


def exercise(label, newline):
    with open(PATH, "r", encoding="utf-8", newline=newline) as handle:
        part = handle.read(2)
        pos = handle.tell()
        rest = handle.read()
        handle.seek(pos)
        rest2 = handle.read()
        handle.seek(0)
        line = handle.readline(3)
        pos2 = handle.tell()
        tail = handle.read()
        handle.seek(pos2)
        tail2 = handle.read()
    print("case", label, "newline", newline)
    print("part", repr(part))
    print("rest", repr(rest), "rest2", repr(rest2))
    print("line", repr(line))
    print("tail", repr(tail), "tail2", repr(tail2))


for newline in (None, ""):
    exercise("utf-8", newline)
