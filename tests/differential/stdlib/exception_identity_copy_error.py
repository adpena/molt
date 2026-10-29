"""Purpose: copy.Error is an ordinary module class and construction has no side effects."""

import copy
import sys


def main():
    print("alias", copy.error is copy.Error)
    print("base", copy.Error.__mro__[1].__name__, copy.Error.__module__)
    error = copy.Error("constructed")
    print("constructed", error.args, sys.exc_info()[0])
    try:
        raise copy.Error("raised")
    except copy.Error as exc:
        print("raised", exc.args, exc.__context__)


if __name__ == "__main__":
    main()
