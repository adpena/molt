"""Purpose: differential coverage for traceback chaining."""

import traceback


def raise_context():
    try:
        1 / 0
    except Exception:
        raise ValueError("context")


def raise_cause():
    try:
        1 / 0
    except Exception as exc:
        raise RuntimeError("cause") from exc


def main():
    try:
        raise_context()
    except Exception as exc:
        text = "".join(traceback.format_exception(type(exc), exc, exc.__traceback__))
        print("context_marker", "During handling of the above exception" in text)

    try:
        raise_cause()
    except Exception as exc:
        text = "".join(traceback.format_exception(type(exc), exc, exc.__traceback__))
        print("cause_marker", "The above exception was the direct cause" in text)

    try:
        raise_cause()
    except Exception as exc:
        tbe = traceback.TracebackException.from_exception(exc)
        lines = list(tbe.format())
        print("tbe_cause_marker", any("direct cause" in line for line in lines))


if __name__ == "__main__":
    main()
