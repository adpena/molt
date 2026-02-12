"""Purpose: differential coverage for UTF-16/UTF-32 text I/O."""

import os


SAMPLE = "alpha\nbeta\r\ngamma\romega"
ROOT = os.path.join("logs", "io_text_encodings")


def run_case(encoding, newline):
    os.makedirs(ROOT, exist_ok=True)
    suffix = encoding.replace("/", "-").replace(" ", "_")
    path = os.path.join(ROOT, f"sample_{suffix}.txt")
    with open(path, "w+", encoding=encoding, newline=newline) as handle:
        handle.write(SAMPLE)
        handle.flush()
        handle.seek(0)
        print("read", encoding, repr(newline), handle.read())
        handle.seek(0)
        print("readline", encoding, repr(newline), handle.readline())
        handle.seek(0)
        print("readlines", encoding, repr(newline), handle.readlines())
    try:
        os.remove(path)
    except OSError:
        pass


def main():
    for enc in ("utf-16", "utf-16-le", "utf-16-be", "utf-32", "utf-32-le", "utf-32-be"):
        run_case(enc, None)
        run_case(enc, "")


if __name__ == "__main__":
    main()
