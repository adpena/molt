"""Purpose: ensure logging percent-style formatting is intrinsic-backed and stable."""

import logging


def main() -> None:
    record = logging.LogRecord("demo", logging.INFO, __file__, 23, "hello", (), None)
    record.custom_int = 7
    record.custom_float = 3.5
    record.custom_obj = {"k": 1}
    fmt = "%(levelname)s|%(custom_int)d|%(custom_float)f|%(custom_obj)r|%%|%(message)s"
    formatter = logging.Formatter(fmt)
    print(formatter.format(record))


if __name__ == "__main__":
    main()
