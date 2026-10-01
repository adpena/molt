"""Diagnostics that avoid exception-controlled attribute and formatting dispatch.

Notes are written to builtin exception storage. A subclass may hide that storage
from its public ``__notes__`` attribute, so True reports storage, not display.
"""

from __future__ import annotations

from typing import cast


def exception_type_name(error: BaseException) -> str:
    """Read the builtin class-name descriptor without metaclass dispatch."""
    try:
        name = cast(str, type.__dict__["__name__"].__get__(type(error), type))
        return str.__str__(name)
    except BaseException:
        return "unknown exception type"


def exception_diagnostic(error: BaseException) -> str:
    """Describe builtin storage, never exception-controlled formatting."""
    try:
        name = exception_type_name(error)
        args = BaseException.__dict__["args"].__get__(error, BaseException)
        strings = (
            [value for value in args if type(value) is str]
            if type(args) is tuple
            else []
        )
        return name + (": " + "; ".join(strings) if strings else "")
    except BaseException:
        return "exception diagnostic unavailable"


def add_exception_note(error: BaseException, note: str) -> bool:
    """Append only to an existing exact list obtained without dictionary lookup.

    Missing, replaced or unsafe storage declines annotation. True reports an
    append to the observed builtin list, not public display or durable storage.
    Callers must retain secondary failures in their own diagnostic records.
    """
    if type(note) is not str:
        return False
    try:
        storage = BaseException.__dict__["__dict__"].__get__(error, BaseException)
        if type(storage) is not dict:
            return False
        for key, notes in dict.items(storage):
            if type(key) is str and key == "__notes__":
                if type(notes) is not list:
                    return False
                list.append(notes, note)
                return True
        return False
    except BaseException:
        return False
