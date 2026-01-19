import pkg_basic.submod as submod

VALUE = 41


def pkg_value() -> int:
    return VALUE + submod.SUBVALUE
