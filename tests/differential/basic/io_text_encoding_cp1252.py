# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
import codecs
import os

ROOT = os.path.join("logs", "io_text_encodings")
os.makedirs(ROOT, exist_ok=True)
PATH = os.path.join(ROOT, "cp1252.txt")

TEXT = "price €"

with open(PATH, "w", encoding="cp1252", newline="") as handle:
    handle.write(TEXT)

with open(PATH, "rb") as handle:
    raw = handle.read()
print("bytes", list(raw))

with open(PATH, "r", encoding="cp1252") as handle:
    print("read", handle.read())

print("lookup", codecs.lookup("windows-1252").name)


def show_error(label, func):
    try:
        func()
        print(label, "ok")
    except Exception as exc:  # pragma: no cover - clarity in differential output
        print(label, type(exc).__name__, str(exc))


show_error("decode_undefined", lambda: codecs.decode(b"\x81", "cp1252"))
print("decode_replace", repr(codecs.decode(b"\x81", "cp1252", "replace")))
show_error("encode_undefined", lambda: codecs.encode("\x81", "cp1252"))
print("encode_euro", codecs.encode("€", "cp1252"))
