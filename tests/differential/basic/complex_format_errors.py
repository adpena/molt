"""Purpose: differential coverage for complex format spec errors."""


def _print_error(label: str, spec: str) -> None:
    try:
        format(1 + 2j, spec)
    except Exception as exc:
        print(label, type(exc).__name__, exc)


_print_error("zero_pad", "010.2f")
_print_error("align_eq", "=10.2f")
_print_error("bad_code", "x")
