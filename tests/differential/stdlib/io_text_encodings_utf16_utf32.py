# MOLT_ENV: PYTHONWARNDEFAULTENCODING=1
"""Purpose: differential coverage for UTF-16/UTF-32 text I/O."""

import io
import locale
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


def locale_selector_case():
    previous = locale.setlocale(locale.LC_CTYPE)
    try:
        locale.setlocale(locale.LC_CTYPE, "C")
        os.makedirs(ROOT, exist_ok=True)
        path = os.path.join(ROOT, "locale.txt")
        for operation in ("open", "wrapper", "reconfigure"):
            if operation == "open":
                stream = open(path, "w", encoding="locale", errors="backslashreplace")
            else:
                stream = io.TextIOWrapper(
                    io.BytesIO(),
                    encoding="locale" if operation == "wrapper" else "utf-8",
                    errors="backslashreplace",
                )
                if operation == "reconfigure":
                    stream.reconfigure(encoding="locale", errors="backslashreplace")
            try:
                print(
                    "locale-label", operation, stream.encoding == locale.getencoding()
                )
                stream.write("caf\N{LATIN SMALL LETTER E WITH ACUTE}")
                stream.flush()
                if operation == "open":
                    with open(path, "rb") as raw:
                        payload = raw.read()
                else:
                    payload = stream.buffer.getvalue()
                print("locale-selector", operation, payload == b"caf\\xe9")
            finally:
                stream.close()
        os.remove(path)
    finally:
        locale.setlocale(locale.LC_CTYPE, previous)


def default_warning_case():
    import warnings

    os.makedirs(ROOT, exist_ok=True)
    path = os.path.join(ROOT, "default-warning.txt")
    with warnings.catch_warnings(record=True) as captured:
        warnings.simplefilter("always", EncodingWarning)
        wrapper = io.TextIOWrapper(io.BytesIO())
        wrapper.close()
        with open(path, "w"):
            pass
        explicit = io.TextIOWrapper(io.BytesIO(), encoding="locale")
        explicit.close()
        print(
            "encoding-warning-count",
            len(captured)
            == (2 if __import__("sys").flags.warn_default_encoding else 0),
        )
    with open(path, "wb") as stream:
        stream.write(b"sentinel")
    with warnings.catch_warnings():
        warnings.simplefilter("error", EncodingWarning)
        try:
            with open(path, "w"):
                pass
        except EncodingWarning:
            with open(path, "rb") as stream:
                print("warning-after-truncate", stream.read() == b"")
        closed = io.BytesIO()
        closed.close()
        try:
            io.TextIOWrapper(closed)
        except Exception as error:
            print("warning-before-buffer", type(error).__name__)
    os.remove(path)


def main():
    default_warning_case()
    locale_selector_case()
    for enc in ("utf-16", "utf-16-le", "utf-16-be", "utf-32", "utf-32-le", "utf-32-be"):
        run_case(enc, None)
        run_case(enc, "")


if __name__ == "__main__":
    main()
