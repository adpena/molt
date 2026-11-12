"""Helper module for module_global_slots_writers: its globals are written
from the importing module, and its ``__getattr__`` answers absent names."""

counter = 0
label = "first"


def bump():
    global counter
    counter += 1
    return counter


def read_counter():
    return counter


def read_label():
    return label


def __getattr__(name):
    if name == "dynamic":
        return "from __getattr__"
    raise AttributeError(name)
