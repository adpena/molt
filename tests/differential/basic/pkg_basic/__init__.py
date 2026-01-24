"""Purpose: differential coverage for package init."""

import pkg_basic.submod as submod

VALUE = 41


def pkg_value() -> int:
    return VALUE + submod.SUBVALUE
