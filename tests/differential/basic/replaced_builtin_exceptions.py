"""Replacing a builtin exception reaches every read and construction (HF-776).

The frontend lowers a proved exception read to the canonical class and a call
of ten of them to one construction. A module binding, a `builtins` member
write and a `__builtins__` mapping each select another object here, so every
read and call must reach it, as in CPython.
"""

import builtins


class Fake:
    def __init__(self, name):
        self.name = name

    def __call__(self, *args):
        return "called-" + self.name

    def __repr__(self):
        return "fake-" + self.name


def show(values):
    return [value if value.__class__ is Fake or value.__class__ is str else value.__name__ for value in values]


def read_all():
    return (
        ArithmeticError,
        AssertionError,
        AttributeError,
        BaseException,
        BaseExceptionGroup,
        BlockingIOError,
        BrokenPipeError,
        BufferError,
        BytesWarning,
        ChildProcessError,
        ConnectionAbortedError,
        ConnectionError,
        ConnectionRefusedError,
        ConnectionResetError,
        DeprecationWarning,
        EOFError,
        EncodingWarning,
        EnvironmentError,
        Exception,
        ExceptionGroup,
        FileExistsError,
        FileNotFoundError,
        FloatingPointError,
        FutureWarning,
        GeneratorExit,
        IOError,
        ImportError,
        ImportWarning,
        IndentationError,
        IndexError,
        InterruptedError,
        IsADirectoryError,
        KeyError,
        KeyboardInterrupt,
        LookupError,
        MemoryError,
        ModuleNotFoundError,
        NameError,
        NotADirectoryError,
        NotImplementedError,
        OSError,
        OverflowError,
        PendingDeprecationWarning,
        PermissionError,
        ProcessLookupError,
        RecursionError,
        ReferenceError,
        ResourceWarning,
        RuntimeError,
        RuntimeWarning,
        StopAsyncIteration,
        StopIteration,
        SyntaxError,
        SyntaxWarning,
        SystemError,
        SystemExit,
        TabError,
        TimeoutError,
        TypeError,
        UnboundLocalError,
        UnicodeDecodeError,
        UnicodeEncodeError,
        UnicodeError,
        UnicodeTranslateError,
        UnicodeWarning,
        UserWarning,
        ValueError,
        Warning,
        ZeroDivisionError,
    )


def call_all():
    return (
        BaseException("x"),
        Exception("x"),
        KeyError("x"),
        IndexError("x"),
        ValueError("x"),
        TypeError("x"),
        RuntimeError("x"),
        StopIteration("x"),
        ExceptionGroup("g", [ValueError("v")]),
        BaseExceptionGroup("b", [KeyError("k")]),
    )


def outcome(values):
    return [value.__class__.__name__ if not isinstance(value, str) else value for value in values]


ORIGINALS = read_all()
print("original", show(ORIGINALS))
print("original calls", outcome(call_all()))
ArithmeticError = Fake("ArithmeticError-module")
AssertionError = Fake("AssertionError-module")
AttributeError = Fake("AttributeError-module")
BaseException = Fake("BaseException-module")
BaseExceptionGroup = Fake("BaseExceptionGroup-module")
BlockingIOError = Fake("BlockingIOError-module")
BrokenPipeError = Fake("BrokenPipeError-module")
BufferError = Fake("BufferError-module")
BytesWarning = Fake("BytesWarning-module")
ChildProcessError = Fake("ChildProcessError-module")
ConnectionAbortedError = Fake("ConnectionAbortedError-module")
ConnectionError = Fake("ConnectionError-module")
ConnectionRefusedError = Fake("ConnectionRefusedError-module")
ConnectionResetError = Fake("ConnectionResetError-module")
DeprecationWarning = Fake("DeprecationWarning-module")
EOFError = Fake("EOFError-module")
EncodingWarning = Fake("EncodingWarning-module")
EnvironmentError = Fake("EnvironmentError-module")
Exception = Fake("Exception-module")
ExceptionGroup = Fake("ExceptionGroup-module")
FileExistsError = Fake("FileExistsError-module")
FileNotFoundError = Fake("FileNotFoundError-module")
FloatingPointError = Fake("FloatingPointError-module")
FutureWarning = Fake("FutureWarning-module")
GeneratorExit = Fake("GeneratorExit-module")
IOError = Fake("IOError-module")
ImportError = Fake("ImportError-module")
ImportWarning = Fake("ImportWarning-module")
IndentationError = Fake("IndentationError-module")
IndexError = Fake("IndexError-module")
InterruptedError = Fake("InterruptedError-module")
IsADirectoryError = Fake("IsADirectoryError-module")
KeyError = Fake("KeyError-module")
KeyboardInterrupt = Fake("KeyboardInterrupt-module")
LookupError = Fake("LookupError-module")
MemoryError = Fake("MemoryError-module")
ModuleNotFoundError = Fake("ModuleNotFoundError-module")
NameError = Fake("NameError-module")
NotADirectoryError = Fake("NotADirectoryError-module")
NotImplementedError = Fake("NotImplementedError-module")
OSError = Fake("OSError-module")
OverflowError = Fake("OverflowError-module")
PendingDeprecationWarning = Fake("PendingDeprecationWarning-module")
PermissionError = Fake("PermissionError-module")
ProcessLookupError = Fake("ProcessLookupError-module")
RecursionError = Fake("RecursionError-module")
ReferenceError = Fake("ReferenceError-module")
ResourceWarning = Fake("ResourceWarning-module")
RuntimeError = Fake("RuntimeError-module")
RuntimeWarning = Fake("RuntimeWarning-module")
StopAsyncIteration = Fake("StopAsyncIteration-module")
StopIteration = Fake("StopIteration-module")
SyntaxError = Fake("SyntaxError-module")
SyntaxWarning = Fake("SyntaxWarning-module")
SystemError = Fake("SystemError-module")
SystemExit = Fake("SystemExit-module")
TabError = Fake("TabError-module")
TimeoutError = Fake("TimeoutError-module")
TypeError = Fake("TypeError-module")
UnboundLocalError = Fake("UnboundLocalError-module")
UnicodeDecodeError = Fake("UnicodeDecodeError-module")
UnicodeEncodeError = Fake("UnicodeEncodeError-module")
UnicodeError = Fake("UnicodeError-module")
UnicodeTranslateError = Fake("UnicodeTranslateError-module")
UnicodeWarning = Fake("UnicodeWarning-module")
UserWarning = Fake("UserWarning-module")
ValueError = Fake("ValueError-module")
Warning = Fake("Warning-module")
ZeroDivisionError = Fake("ZeroDivisionError-module")
print("module binding", show(read_all()))
print("module binding calls", outcome(call_all()))
del ArithmeticError
del AssertionError
del AttributeError
del BaseException
del BaseExceptionGroup
del BlockingIOError
del BrokenPipeError
del BufferError
del BytesWarning
del ChildProcessError
del ConnectionAbortedError
del ConnectionError
del ConnectionRefusedError
del ConnectionResetError
del DeprecationWarning
del EOFError
del EncodingWarning
del EnvironmentError
del Exception
del ExceptionGroup
del FileExistsError
del FileNotFoundError
del FloatingPointError
del FutureWarning
del GeneratorExit
del IOError
del ImportError
del ImportWarning
del IndentationError
del IndexError
del InterruptedError
del IsADirectoryError
del KeyError
del KeyboardInterrupt
del LookupError
del MemoryError
del ModuleNotFoundError
del NameError
del NotADirectoryError
del NotImplementedError
del OSError
del OverflowError
del PendingDeprecationWarning
del PermissionError
del ProcessLookupError
del RecursionError
del ReferenceError
del ResourceWarning
del RuntimeError
del RuntimeWarning
del StopAsyncIteration
del StopIteration
del SyntaxError
del SyntaxWarning
del SystemError
del SystemExit
del TabError
del TimeoutError
del TypeError
del UnboundLocalError
del UnicodeDecodeError
del UnicodeEncodeError
del UnicodeError
del UnicodeTranslateError
del UnicodeWarning
del UserWarning
del ValueError
del Warning
del ZeroDivisionError
print("module binding deleted", show(read_all()))
builtins.ArithmeticError = Fake("ArithmeticError-member")
builtins.AssertionError = Fake("AssertionError-member")
builtins.AttributeError = Fake("AttributeError-member")
builtins.BaseException = Fake("BaseException-member")
builtins.BaseExceptionGroup = Fake("BaseExceptionGroup-member")
builtins.BlockingIOError = Fake("BlockingIOError-member")
builtins.BrokenPipeError = Fake("BrokenPipeError-member")
builtins.BufferError = Fake("BufferError-member")
builtins.BytesWarning = Fake("BytesWarning-member")
builtins.ChildProcessError = Fake("ChildProcessError-member")
builtins.ConnectionAbortedError = Fake("ConnectionAbortedError-member")
builtins.ConnectionError = Fake("ConnectionError-member")
builtins.ConnectionRefusedError = Fake("ConnectionRefusedError-member")
builtins.ConnectionResetError = Fake("ConnectionResetError-member")
builtins.DeprecationWarning = Fake("DeprecationWarning-member")
builtins.EOFError = Fake("EOFError-member")
builtins.EncodingWarning = Fake("EncodingWarning-member")
builtins.EnvironmentError = Fake("EnvironmentError-member")
builtins.Exception = Fake("Exception-member")
builtins.ExceptionGroup = Fake("ExceptionGroup-member")
builtins.FileExistsError = Fake("FileExistsError-member")
builtins.FileNotFoundError = Fake("FileNotFoundError-member")
builtins.FloatingPointError = Fake("FloatingPointError-member")
builtins.FutureWarning = Fake("FutureWarning-member")
builtins.GeneratorExit = Fake("GeneratorExit-member")
builtins.IOError = Fake("IOError-member")
builtins.ImportError = Fake("ImportError-member")
builtins.ImportWarning = Fake("ImportWarning-member")
builtins.IndentationError = Fake("IndentationError-member")
builtins.IndexError = Fake("IndexError-member")
builtins.InterruptedError = Fake("InterruptedError-member")
builtins.IsADirectoryError = Fake("IsADirectoryError-member")
builtins.KeyError = Fake("KeyError-member")
builtins.KeyboardInterrupt = Fake("KeyboardInterrupt-member")
builtins.LookupError = Fake("LookupError-member")
builtins.MemoryError = Fake("MemoryError-member")
builtins.ModuleNotFoundError = Fake("ModuleNotFoundError-member")
builtins.NameError = Fake("NameError-member")
builtins.NotADirectoryError = Fake("NotADirectoryError-member")
builtins.NotImplementedError = Fake("NotImplementedError-member")
builtins.OSError = Fake("OSError-member")
builtins.OverflowError = Fake("OverflowError-member")
builtins.PendingDeprecationWarning = Fake("PendingDeprecationWarning-member")
builtins.PermissionError = Fake("PermissionError-member")
builtins.ProcessLookupError = Fake("ProcessLookupError-member")
builtins.RecursionError = Fake("RecursionError-member")
builtins.ReferenceError = Fake("ReferenceError-member")
builtins.ResourceWarning = Fake("ResourceWarning-member")
builtins.RuntimeError = Fake("RuntimeError-member")
builtins.RuntimeWarning = Fake("RuntimeWarning-member")
builtins.StopAsyncIteration = Fake("StopAsyncIteration-member")
builtins.StopIteration = Fake("StopIteration-member")
builtins.SyntaxError = Fake("SyntaxError-member")
builtins.SyntaxWarning = Fake("SyntaxWarning-member")
builtins.SystemError = Fake("SystemError-member")
builtins.SystemExit = Fake("SystemExit-member")
builtins.TabError = Fake("TabError-member")
builtins.TimeoutError = Fake("TimeoutError-member")
builtins.TypeError = Fake("TypeError-member")
builtins.UnboundLocalError = Fake("UnboundLocalError-member")
builtins.UnicodeDecodeError = Fake("UnicodeDecodeError-member")
builtins.UnicodeEncodeError = Fake("UnicodeEncodeError-member")
builtins.UnicodeError = Fake("UnicodeError-member")
builtins.UnicodeTranslateError = Fake("UnicodeTranslateError-member")
builtins.UnicodeWarning = Fake("UnicodeWarning-member")
builtins.UserWarning = Fake("UserWarning-member")
builtins.ValueError = Fake("ValueError-member")
builtins.Warning = Fake("Warning-member")
builtins.ZeroDivisionError = Fake("ZeroDivisionError-member")
print("member", show(read_all()))
print("member calls", outcome(call_all()))
builtins.ArithmeticError = ORIGINALS[0]
builtins.AssertionError = ORIGINALS[1]
builtins.AttributeError = ORIGINALS[2]
builtins.BaseException = ORIGINALS[3]
builtins.BaseExceptionGroup = ORIGINALS[4]
builtins.BlockingIOError = ORIGINALS[5]
builtins.BrokenPipeError = ORIGINALS[6]
builtins.BufferError = ORIGINALS[7]
builtins.BytesWarning = ORIGINALS[8]
builtins.ChildProcessError = ORIGINALS[9]
builtins.ConnectionAbortedError = ORIGINALS[10]
builtins.ConnectionError = ORIGINALS[11]
builtins.ConnectionRefusedError = ORIGINALS[12]
builtins.ConnectionResetError = ORIGINALS[13]
builtins.DeprecationWarning = ORIGINALS[14]
builtins.EOFError = ORIGINALS[15]
builtins.EncodingWarning = ORIGINALS[16]
builtins.EnvironmentError = ORIGINALS[17]
builtins.Exception = ORIGINALS[18]
builtins.ExceptionGroup = ORIGINALS[19]
builtins.FileExistsError = ORIGINALS[20]
builtins.FileNotFoundError = ORIGINALS[21]
builtins.FloatingPointError = ORIGINALS[22]
builtins.FutureWarning = ORIGINALS[23]
builtins.GeneratorExit = ORIGINALS[24]
builtins.IOError = ORIGINALS[25]
builtins.ImportError = ORIGINALS[26]
builtins.ImportWarning = ORIGINALS[27]
builtins.IndentationError = ORIGINALS[28]
builtins.IndexError = ORIGINALS[29]
builtins.InterruptedError = ORIGINALS[30]
builtins.IsADirectoryError = ORIGINALS[31]
builtins.KeyError = ORIGINALS[32]
builtins.KeyboardInterrupt = ORIGINALS[33]
builtins.LookupError = ORIGINALS[34]
builtins.MemoryError = ORIGINALS[35]
builtins.ModuleNotFoundError = ORIGINALS[36]
builtins.NameError = ORIGINALS[37]
builtins.NotADirectoryError = ORIGINALS[38]
builtins.NotImplementedError = ORIGINALS[39]
builtins.OSError = ORIGINALS[40]
builtins.OverflowError = ORIGINALS[41]
builtins.PendingDeprecationWarning = ORIGINALS[42]
builtins.PermissionError = ORIGINALS[43]
builtins.ProcessLookupError = ORIGINALS[44]
builtins.RecursionError = ORIGINALS[45]
builtins.ReferenceError = ORIGINALS[46]
builtins.ResourceWarning = ORIGINALS[47]
builtins.RuntimeError = ORIGINALS[48]
builtins.RuntimeWarning = ORIGINALS[49]
builtins.StopAsyncIteration = ORIGINALS[50]
builtins.StopIteration = ORIGINALS[51]
builtins.SyntaxError = ORIGINALS[52]
builtins.SyntaxWarning = ORIGINALS[53]
builtins.SystemError = ORIGINALS[54]
builtins.SystemExit = ORIGINALS[55]
builtins.TabError = ORIGINALS[56]
builtins.TimeoutError = ORIGINALS[57]
builtins.TypeError = ORIGINALS[58]
builtins.UnboundLocalError = ORIGINALS[59]
builtins.UnicodeDecodeError = ORIGINALS[60]
builtins.UnicodeEncodeError = ORIGINALS[61]
builtins.UnicodeError = ORIGINALS[62]
builtins.UnicodeTranslateError = ORIGINALS[63]
builtins.UnicodeWarning = ORIGINALS[64]
builtins.UserWarning = ORIGINALS[65]
builtins.ValueError = ORIGINALS[66]
builtins.Warning = ORIGINALS[67]
builtins.ZeroDivisionError = ORIGINALS[68]
print("member restored", show(read_all()))
custom = dict(builtins.__dict__)
custom["ArithmeticError"] = Fake("ArithmeticError-mapping")
custom["AssertionError"] = Fake("AssertionError-mapping")
custom["AttributeError"] = Fake("AttributeError-mapping")
custom["BaseException"] = Fake("BaseException-mapping")
custom["BaseExceptionGroup"] = Fake("BaseExceptionGroup-mapping")
custom["BlockingIOError"] = Fake("BlockingIOError-mapping")
custom["BrokenPipeError"] = Fake("BrokenPipeError-mapping")
custom["BufferError"] = Fake("BufferError-mapping")
custom["BytesWarning"] = Fake("BytesWarning-mapping")
custom["ChildProcessError"] = Fake("ChildProcessError-mapping")
custom["ConnectionAbortedError"] = Fake("ConnectionAbortedError-mapping")
custom["ConnectionError"] = Fake("ConnectionError-mapping")
custom["ConnectionRefusedError"] = Fake("ConnectionRefusedError-mapping")
custom["ConnectionResetError"] = Fake("ConnectionResetError-mapping")
custom["DeprecationWarning"] = Fake("DeprecationWarning-mapping")
custom["EOFError"] = Fake("EOFError-mapping")
custom["EncodingWarning"] = Fake("EncodingWarning-mapping")
custom["EnvironmentError"] = Fake("EnvironmentError-mapping")
custom["Exception"] = Fake("Exception-mapping")
custom["ExceptionGroup"] = Fake("ExceptionGroup-mapping")
custom["FileExistsError"] = Fake("FileExistsError-mapping")
custom["FileNotFoundError"] = Fake("FileNotFoundError-mapping")
custom["FloatingPointError"] = Fake("FloatingPointError-mapping")
custom["FutureWarning"] = Fake("FutureWarning-mapping")
custom["GeneratorExit"] = Fake("GeneratorExit-mapping")
custom["IOError"] = Fake("IOError-mapping")
custom["ImportError"] = Fake("ImportError-mapping")
custom["ImportWarning"] = Fake("ImportWarning-mapping")
custom["IndentationError"] = Fake("IndentationError-mapping")
custom["IndexError"] = Fake("IndexError-mapping")
custom["InterruptedError"] = Fake("InterruptedError-mapping")
custom["IsADirectoryError"] = Fake("IsADirectoryError-mapping")
custom["KeyError"] = Fake("KeyError-mapping")
custom["KeyboardInterrupt"] = Fake("KeyboardInterrupt-mapping")
custom["LookupError"] = Fake("LookupError-mapping")
custom["MemoryError"] = Fake("MemoryError-mapping")
custom["ModuleNotFoundError"] = Fake("ModuleNotFoundError-mapping")
custom["NameError"] = Fake("NameError-mapping")
custom["NotADirectoryError"] = Fake("NotADirectoryError-mapping")
custom["NotImplementedError"] = Fake("NotImplementedError-mapping")
custom["OSError"] = Fake("OSError-mapping")
custom["OverflowError"] = Fake("OverflowError-mapping")
custom["PendingDeprecationWarning"] = Fake("PendingDeprecationWarning-mapping")
custom["PermissionError"] = Fake("PermissionError-mapping")
custom["ProcessLookupError"] = Fake("ProcessLookupError-mapping")
custom["RecursionError"] = Fake("RecursionError-mapping")
custom["ReferenceError"] = Fake("ReferenceError-mapping")
custom["ResourceWarning"] = Fake("ResourceWarning-mapping")
custom["RuntimeError"] = Fake("RuntimeError-mapping")
custom["RuntimeWarning"] = Fake("RuntimeWarning-mapping")
custom["StopAsyncIteration"] = Fake("StopAsyncIteration-mapping")
custom["StopIteration"] = Fake("StopIteration-mapping")
custom["SyntaxError"] = Fake("SyntaxError-mapping")
custom["SyntaxWarning"] = Fake("SyntaxWarning-mapping")
custom["SystemError"] = Fake("SystemError-mapping")
custom["SystemExit"] = Fake("SystemExit-mapping")
custom["TabError"] = Fake("TabError-mapping")
custom["TimeoutError"] = Fake("TimeoutError-mapping")
custom["TypeError"] = Fake("TypeError-mapping")
custom["UnboundLocalError"] = Fake("UnboundLocalError-mapping")
custom["UnicodeDecodeError"] = Fake("UnicodeDecodeError-mapping")
custom["UnicodeEncodeError"] = Fake("UnicodeEncodeError-mapping")
custom["UnicodeError"] = Fake("UnicodeError-mapping")
custom["UnicodeTranslateError"] = Fake("UnicodeTranslateError-mapping")
custom["UnicodeWarning"] = Fake("UnicodeWarning-mapping")
custom["UserWarning"] = Fake("UserWarning-mapping")
custom["ValueError"] = Fake("ValueError-mapping")
custom["Warning"] = Fake("Warning-mapping")
custom["ZeroDivisionError"] = Fake("ZeroDivisionError-mapping")
__builtins__ = custom


def read_after():
    return (
        ArithmeticError,
        AssertionError,
        AttributeError,
        BaseException,
        BaseExceptionGroup,
        BlockingIOError,
        BrokenPipeError,
        BufferError,
        BytesWarning,
        ChildProcessError,
        ConnectionAbortedError,
        ConnectionError,
        ConnectionRefusedError,
        ConnectionResetError,
        DeprecationWarning,
        EOFError,
        EncodingWarning,
        EnvironmentError,
        Exception,
        ExceptionGroup,
        FileExistsError,
        FileNotFoundError,
        FloatingPointError,
        FutureWarning,
        GeneratorExit,
        IOError,
        ImportError,
        ImportWarning,
        IndentationError,
        IndexError,
        InterruptedError,
        IsADirectoryError,
        KeyError,
        KeyboardInterrupt,
        LookupError,
        MemoryError,
        ModuleNotFoundError,
        NameError,
        NotADirectoryError,
        NotImplementedError,
        OSError,
        OverflowError,
        PendingDeprecationWarning,
        PermissionError,
        ProcessLookupError,
        RecursionError,
        ReferenceError,
        ResourceWarning,
        RuntimeError,
        RuntimeWarning,
        StopAsyncIteration,
        StopIteration,
        SyntaxError,
        SyntaxWarning,
        SystemError,
        SystemExit,
        TabError,
        TimeoutError,
        TypeError,
        UnboundLocalError,
        UnicodeDecodeError,
        UnicodeEncodeError,
        UnicodeError,
        UnicodeTranslateError,
        UnicodeWarning,
        UserWarning,
        ValueError,
        Warning,
        ZeroDivisionError,
    )


def call_after():
    return (
        BaseException("x"),
        Exception("x"),
        KeyError("x"),
        IndexError("x"),
        ValueError("x"),
        TypeError("x"),
        RuntimeError("x"),
        StopIteration("x"),
        ExceptionGroup("g", [ValueError("v")]),
        BaseExceptionGroup("b", [KeyError("k")]),
    )


print("mapping", show(read_all()), show(read_after()))
print("mapping calls", outcome(call_all()), outcome(call_after()))
