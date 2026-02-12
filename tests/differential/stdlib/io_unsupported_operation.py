"""Purpose: differential coverage for io unsupported operation."""


def _report(label, value):
    print(f"{label}:{value}")


def main():
    import io

    _report("io_has_unsupported", hasattr(io, "UnsupportedOperation"))
    _report("io_unsupported_is_type", isinstance(io.UnsupportedOperation, type))
    _report("io_unsupported_base_oserror", issubclass(io.UnsupportedOperation, OSError))
    _report(
        "io_unsupported_base_valueerror",
        issubclass(io.UnsupportedOperation, ValueError),
    )
    try:
        raise io.UnsupportedOperation("not readable")
    except io.UnsupportedOperation as exc:
        _report("io_unsupported_exc_name", type(exc).__name__)
        _report("io_unsupported_exc_msg", str(exc))
    except Exception as exc:
        _report("io_unsupported_exc_name", type(exc).__name__)
        _report("io_unsupported_exc_msg", str(exc))
    else:
        _report("io_unsupported_exc_name", "missing")


if __name__ == "__main__":
    main()
