# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,os.getpid
"""Purpose: logging must share the canonical Python string-formatting contract."""

import logging
import time


def main() -> None:
    record = logging.LogRecord("demo", logging.INFO, __file__, 23, "hello", (), None)
    record.custom_int = 7
    record.custom_float = 3.5
    record.custom_obj = {"k": 1}
    fmt = "%(levelname)s|%(custom_int)d|%(custom_float)f|%(custom_obj)r|%%|%(message)s"
    formatter = logging.Formatter(fmt)
    print(formatter.format(record))

    record.name, record.levelname = "renamed", "CUSTOM"
    record.msg, record.args = "updated %s", ("value",)
    for style, fmt in (
        ("%", "%(name)s|%(levelname)s|%(message)s"),
        ("{", "{name}|{levelname}|{message}"),
        ("$", "$name|$levelname|$message"),
    ):
        formatter = logging.Formatter(fmt, style=style)
        print("live", style, formatter.format(record), record.message)

    for style, fmt in (("%", "%(missing)s"), ("{", "{missing}"), ("$", "$missing")):
        try:
            logging.Formatter(fmt, style=style).format(record)
        except ValueError as exc:
            print("missing", style, str(exc), type(exc.__context__).__name__)

    record.created, record.msecs = 0, 7
    formatter = logging.Formatter("%(asctime)s|%(message)s")
    formatter.converter = time.gmtime
    print("time", formatter.format(record))
    record.created, record.msecs = 86400, 91
    print("time changed", formatter.format(record))
    formatter.default_time_format, formatter.default_msec_format = "%Y-%m-%d", None
    print("time policy", formatter.format(record))

    mapped = logging.LogRecord("mapping", logging.INFO, "fixture", 1,
                               "%(value)s", ({"value": "mapped"},), None)
    print("mapping", mapped.getMessage())
    mapped.msg, mapped.args = None, ()
    print("none message", mapped.getMessage())
    try:
        logging.Handler().emit(record)
    except NotImplementedError:
        print("abstract handler")


if __name__ == "__main__":
    main()
