"""Purpose: differential coverage for raise from None with finally returns."""


def run():
    try:
        try:
            raise KeyError("inner")
        except Exception:
            raise ValueError("mid") from None
        finally:
            return "finally"
    except Exception as exc:
        return f"exc:{type(exc).__name__}"


print("result", run())
