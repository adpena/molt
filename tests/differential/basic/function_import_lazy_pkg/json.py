"""A package submodule that shares its last name with the standard `json`."""


def dumps(value):
    return "package json: " + repr(value)
