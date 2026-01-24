"""Purpose: differential coverage for mutable default arguments."""


def f(value, acc=[]):
    acc.append(value)
    return list(acc)


if __name__ == "__main__":
    print("first", f(1))
    print("second", f(2))
