"""Purpose: differential coverage for PEP 701 f-string full grammar."""


def main() -> None:
    value = 2
    nested = f"{f'{value + 1}'}"
    commented = f"{(1 + 2  # comment inside f-string expression\n    )}"
    continued = f"{(1 + \\\n    2)}"
    debug = f"{value=}"
    print(nested)
    print(commented)
    print(continued)
    print(debug)


main()
