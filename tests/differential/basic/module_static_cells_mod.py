"""Helper for module_static_cells.py: a module whose globals are static cells
(no code reaches its dictionary by identity)."""

value = 1
label = "first"


def read_value():
    return value


def read_label():
    return label


def __getattr__(name):
    return "hook:" + name
