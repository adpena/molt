"""Purpose: list.append() inside an `except` block must mutate the list.

Output must be byte-identical to CPython 3.14.
"""


def collect_errors() -> list:
    results: list = []
    for i in range(5):
        try:
            if i % 2 == 0:
                raise ValueError("even " + str(i))
            results.append(("ok", i))
        except ValueError as e:
            results.append(("err", str(e)))
    return results


def append_in_except_simple() -> list:
    log: list = []
    try:
        raise RuntimeError("boom")
    except RuntimeError as e:
        log.append("caught:" + str(e))
        log.append("second")
    return log


def nested_append() -> list:
    out: list = []
    for x in range(3):
        try:
            try:
                raise KeyError(x)
            except KeyError:
                out.append("inner:" + str(x))
                raise IndexError(x)
        except IndexError:
            out.append("outer:" + str(x))
    return out


def main() -> None:
    print(collect_errors())
    print(append_in_except_simple())
    print(nested_append())


if __name__ == "__main__":
    main()
