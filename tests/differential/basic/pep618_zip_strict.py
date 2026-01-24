"""Purpose: differential coverage for PEP 618 zip(strict=...)."""


def show_zip(left, right) -> None:
    try:
        print(list(zip(left, right, strict=True)))
    except Exception as exc:
        print(type(exc).__name__, exc)


show_zip([1, 2], [3, 4])
show_zip([1, 2, 3], [4, 5])
show_zip([1], [2, 3])
