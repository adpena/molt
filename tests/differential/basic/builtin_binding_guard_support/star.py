"""A star import that shadows a builtin in the importing module."""

__all__ = ["len"]


def len(value):
    return 42
