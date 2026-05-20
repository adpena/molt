"""Purpose: differential coverage for exception message."""


def header(name: str) -> None:
    print(f"-- {name} --")


header("valueerror_int")
try:
    raise ValueError(1)
except ValueError as exc:
    print(str(exc))

header("exception_list")
try:
    raise Exception([1, 2])
except Exception as exc:
    print(str(exc))

header("custom_str")


class CustomStrError(ValueError):
    def __str__(self):
        return "custom:" + self.args[0]


try:
    raise CustomStrError("payload")
except CustomStrError as exc:
    print(str(exc))
