"""User module spelling does not determine Python frame ownership."""

marker = "lexical"


def probe(value):
    global marker
    marker = value
    return marker, globals(), locals()["value"]


def fail():
    raise ValueError("ordinary-function-failure")


def annotated(flag):
    class Inner:
        if flag:
            field: int = 1

    return Inner
