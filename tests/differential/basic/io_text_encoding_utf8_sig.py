# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
import os

ROOT = os.path.join("logs", "io_text_encodings")
os.makedirs(ROOT, exist_ok=True)
PATH = os.path.join(ROOT, "utf8_sig.txt")

SAMPLE = "alpha\nbeta\r\ngamma\romega"

with open(PATH, "w", encoding="utf-8-sig", newline="") as handle:
    handle.write("")

with open(PATH, "rb") as handle:
    raw = handle.read()
print("empty_bytes", list(raw))

with open(PATH, "w", encoding="utf-8-sig", newline="") as handle:
    handle.write(SAMPLE)

with open(PATH, "rb") as handle:
    raw = handle.read()
print("bytes_prefix", list(raw[:6]))
print("bytes_len", len(raw))

for newline in (None, ""):
    with open(PATH, "r", encoding="utf-8-sig", newline=newline) as handle:
        print("read", newline, handle.read())
        handle.seek(0)
        print("readline", newline, handle.readline())
        handle.seek(0)
        print("readlines", newline, handle.readlines())
