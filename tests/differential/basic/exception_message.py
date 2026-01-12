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
