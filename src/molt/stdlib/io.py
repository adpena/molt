"""Public I/O ABCs over Molt's native storage and protocol authority."""

import _io
import abc
from _io import *

UnsupportedOperation.__module__ = "io"


class IOBase(_io._IOBase, metaclass=abc.ABCMeta):
    __doc__ = _io._IOBase.__doc__


class RawIOBase(_io._RawIOBase, IOBase):
    __doc__ = _io._RawIOBase.__doc__


class BufferedIOBase(_io._BufferedIOBase, IOBase):
    __doc__ = _io._BufferedIOBase.__doc__


class TextIOBase(_io._TextIOBase, IOBase):
    __doc__ = _io._TextIOBase.__doc__


RawIOBase.register(FileIO)
for _class in (BytesIO, BufferedReader, BufferedWriter, BufferedRandom):
    BufferedIOBase.register(_class)
for _class in (StringIO, TextIOWrapper):
    TextIOBase.register(_class)
del _class

__all__ = _io.__all__ + ["IOBase", "RawIOBase", "BufferedIOBase", "TextIOBase"]
