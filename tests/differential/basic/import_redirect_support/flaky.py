"""A module whose first import fails after its globals are partly bound."""

from import_redirect_support import control

try:
    runs  # noqa: B018 - a global read before this run binds it
except NameError:
    runs = 0
runs += 1
counter = runs * 10
if control.fail_next:
    control.fail_next = False
    raise RuntimeError("first import fails")


def get() -> int:
    return counter
