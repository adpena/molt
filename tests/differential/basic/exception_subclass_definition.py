"""Purpose: verify user-defined exception subclasses of builtin exceptions."""


def main():
    class MyValueError(ValueError):
        pass

    class MyTypeError(TypeError):
        pass

    class MyRuntimeError(RuntimeError):
        pass

    class MyKeyError(KeyError):
        pass

    class MyOSError(OSError):
        pass

    class MyLookupError(LookupError):
        pass

    # Check class hierarchy
    print("MyValueError bases:", MyValueError.__bases__)
    print("issubclass(MyValueError, ValueError):", issubclass(MyValueError, ValueError))
    print("issubclass(MyValueError, Exception):", issubclass(MyValueError, Exception))
    print(
        "issubclass(MyValueError, BaseException):",
        issubclass(MyValueError, BaseException),
    )

    print("MyTypeError bases:", MyTypeError.__bases__)
    print("issubclass(MyTypeError, TypeError):", issubclass(MyTypeError, TypeError))

    print("MyRuntimeError bases:", MyRuntimeError.__bases__)
    print(
        "issubclass(MyRuntimeError, RuntimeError):",
        issubclass(MyRuntimeError, RuntimeError),
    )

    print("MyKeyError bases:", MyKeyError.__bases__)
    print("issubclass(MyKeyError, KeyError):", issubclass(MyKeyError, KeyError))

    # Test instantiation and raising
    try:
        raise MyValueError("test error")
    except ValueError as e:
        print("caught MyValueError as ValueError:", type(e).__name__)

    try:
        raise MyTypeError("type error")
    except TypeError as e:
        print("caught MyTypeError as TypeError:", type(e).__name__)

    # Test custom __init__
    class DetailedError(ValueError):
        def __init__(self, msg, code):
            self.code = code
            super().__init__(msg)

    e = DetailedError("bad value", 42)
    print("DetailedError.code:", e.code)
    print("isinstance(e, ValueError):", isinstance(e, ValueError))

    # Test multi-level subclassing
    class SpecificError(MyValueError):
        pass

    print(
        "issubclass(SpecificError, ValueError):",
        issubclass(SpecificError, ValueError),
    )
    print(
        "issubclass(SpecificError, Exception):", issubclass(SpecificError, Exception)
    )


if __name__ == "__main__":
    main()
