import os


path = "_molt_io_isatty.txt"
with open(path, "w", encoding="utf-8") as handle:
    handle.write("hello")

try:
    with open(path, "r", encoding="utf-8") as handle:
        print("isatty-file", handle.isatty())
finally:
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass
