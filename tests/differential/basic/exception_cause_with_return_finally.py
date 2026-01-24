"""Purpose: differential coverage for explicit cause with finally return override."""


def run():
    try:
        try:
            raise KeyError("inner")
        except Exception as exc:
            raise ValueError("mid") from exc
    finally:
        return "finally"


print("result", run())
