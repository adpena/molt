"""Purpose: differential coverage for decimal as_tuple/normalize."""

from decimal import Decimal


def main():
    value = Decimal("123.4500")
    print("tuple", value.as_tuple())
    print("normalize", value.normalize())


if __name__ == "__main__":
    main()
