"""Purpose: differential coverage for decimal flags clearing."""

from decimal import Decimal, Inexact, getcontext


def main():
    ctx = getcontext()
    ctx.clear_flags()
    Decimal("1") / Decimal("3")
    print("flagged", ctx.flags[Inexact])
    ctx.clear_flags()
    print("cleared", ctx.flags[Inexact])


if __name__ == "__main__":
    main()
