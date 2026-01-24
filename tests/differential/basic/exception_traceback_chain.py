"""Purpose: differential coverage for exception __traceback__ chain."""


def inner():
    raise ValueError("inner")


def outer():
    try:
        inner()
    except ValueError as exc:
        raise RuntimeError("outer") from exc


if __name__ == "__main__":
    try:
        outer()
    except Exception as exc:
        tb = exc.__traceback__
        names = []
        while tb is not None:
            names.append(tb.tb_frame.f_code.co_name)
            tb = tb.tb_next
        print("trace", names)
        print("cause", type(exc.__cause__).__name__)
