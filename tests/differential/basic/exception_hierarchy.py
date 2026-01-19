def _report(label, value):
    print(f"{label}:{value}")


def _check_sub(label, cls, base):
    _report(label, issubclass(cls, base))


def main():
    _check_sub("IndexError->LookupError", IndexError, LookupError)
    _check_sub("KeyError->LookupError", KeyError, LookupError)
    _check_sub("LookupError->Exception", LookupError, Exception)
    _check_sub("KeyboardInterrupt->Exception", KeyboardInterrupt, Exception)
    _check_sub("KeyboardInterrupt->BaseException", KeyboardInterrupt, BaseException)
    _check_sub("SystemExit->BaseException", SystemExit, BaseException)
    _check_sub("BlockingIOError->OSError", BlockingIOError, OSError)
    _check_sub(
        "ConnectionResetError->ConnectionError", ConnectionResetError, ConnectionError
    )
    _check_sub("ConnectionError->OSError", ConnectionError, OSError)
    _check_sub("UnicodeDecodeError->UnicodeError", UnicodeDecodeError, UnicodeError)
    _check_sub("UnicodeError->ValueError", UnicodeError, ValueError)
    _check_sub("NotImplementedError->RuntimeError", NotImplementedError, RuntimeError)
    _check_sub("RecursionError->RuntimeError", RecursionError, RuntimeError)
    _check_sub("IndentationError->SyntaxError", IndentationError, SyntaxError)
    _check_sub("TabError->IndentationError", TabError, IndentationError)
    _check_sub("EncodingWarning->Warning", EncodingWarning, Warning)
    _check_sub("ResourceWarning->Warning", ResourceWarning, Warning)
    _check_sub("ExceptionGroup->Exception", ExceptionGroup, Exception)
    _check_sub("ExceptionGroup->BaseExceptionGroup", ExceptionGroup, BaseExceptionGroup)
    _check_sub("BaseExceptionGroup->BaseException", BaseExceptionGroup, BaseException)
    _report("IOError_is_OSError", IOError is OSError)
    _report("EnvironmentError_is_OSError", EnvironmentError is OSError)


if __name__ == "__main__":
    main()
