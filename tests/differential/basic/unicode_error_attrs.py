"""Purpose: UnicodeError attribute and validation parity."""


def show(label, exc):
    print(
        f"{label}:{exc.encoding}:{exc.object}:{exc.start}:{exc.end}:{exc.reason}"
    )


show("encode", UnicodeEncodeError("utf-8", "Ω", 0, 1, "reason"))
show("decode", UnicodeDecodeError("utf-8", b"\xff", 0, 1, "reason"))
show("translate", UnicodeTranslateError("abc", 0, 1, "reason"))


def check(label, thunk):
    try:
        thunk()
    except Exception as exc:  # pragma: no cover - printed output is the check
        print(f"{label}:{type(exc).__name__}:{exc}")


check("encode-argc", lambda: UnicodeEncodeError("utf-8", "x", 0, 1))
check("translate-argc", lambda: UnicodeTranslateError("x", 0, 1))
check(
    "encode-enc-type",
    lambda: UnicodeEncodeError(123, "x", 0, 1, "reason"),
)
check(
    "decode-enc-type",
    lambda: UnicodeDecodeError(123, b"x", 0, 1, "reason"),
)
check(
    "encode-obj-type",
    lambda: UnicodeEncodeError("utf-8", 123, 0, 1, "reason"),
)
check(
    "decode-obj-type",
    lambda: UnicodeDecodeError("utf-8", "x", 0, 1, "reason"),
)
check(
    "translate-obj-type",
    lambda: UnicodeTranslateError(123, 0, 1, "reason"),
)
check(
    "encode-reason-type",
    lambda: UnicodeEncodeError("utf-8", "x", 0, 1, 123),
)
check(
    "translate-reason-type",
    lambda: UnicodeTranslateError("x", 0, 1, 123),
)
check(
    "encode-start-type",
    lambda: UnicodeEncodeError("utf-8", "x", "bad", 1, "reason"),
)
check(
    "encode-overflow",
    lambda: UnicodeEncodeError("utf-8", "x", 10**100, 10**100 + 1, "reason"),
)
