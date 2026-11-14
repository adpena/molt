"""Helper for module_static_cells_reflection.py: another module reaches this
module's dictionary, so it keeps its dictionary namespace."""

value = 1


def read_value():
    return value
