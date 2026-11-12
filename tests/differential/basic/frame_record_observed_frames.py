"""Purpose: a frame observed while it runs keeps its bindings, line and caller after it returns."""

import sys
import warnings


def keeps_frame(value):
    local = value * 2
    frame = sys._getframe(0)
    return frame


def outer_keeps(value):
    frame = keeps_frame(value)
    return frame


def generator():
    held = "generator local"
    yield held
    yield held.upper()


def make_suspended():
    gen = generator()
    first = next(gen)
    frame = gen.gi_frame
    return gen, frame, first


def warn_at(level):
    warnings.warn("observed", UserWarning, stacklevel=level)


def warn_middle(level):
    warn_at(level)


def warn_outer(level):
    warn_middle(level)


if __name__ == "__main__":
    frame = outer_keeps(21)
    print("retired", frame.f_code.co_name, sorted(frame.f_locals), frame.f_locals["local"])
    print("back", frame.f_back.f_code.co_name, frame.f_back.f_back.f_code.co_name)
    print("line", frame.f_lineno > 0)

    gen, gi_frame, first = make_suspended()
    print("suspended", first, gi_frame.f_code.co_name, gi_frame.f_locals["held"])
    print("resumed", next(gen), gi_frame.f_locals["held"])

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        for level in (1, 2, 3):
            warn_outer(level)
    for warning in caught:
        print("warning line", warning.lineno > 0, warning.filename.endswith(".py"))
