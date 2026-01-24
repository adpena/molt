"""Purpose: differential coverage for string format errors."""
# ruff: noqa: F521, F523, F524


def _print_error(label: str, func) -> None:
    try:
        func()
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, exc)


def main() -> None:
    _print_error("single_open", lambda: "{".format(1))
    _print_error("single_close", lambda: "}".format(1))
    _print_error("unclosed", lambda: "{0".format(1))
    _print_error("bad_conv", lambda: "{0!x}".format(1))
    _print_error("no_args", lambda: "{0}".format())
    _print_error("missing_kw", lambda: "{foo}".format())
    _print_error("bad_follow", lambda: "{0[1]x}".format([1, 2]))
    _print_error("empty_attr", lambda: "{0.}".format(object()))
    _print_error("big_index", lambda: "{999999999999999999999999999999}".format(1))
    _print_error("spec_unmatched", lambda: "{0:{".format(1))
    _print_error("conv_unmatched", lambda: "{!}".format(1))


if __name__ == "__main__":
    main()
