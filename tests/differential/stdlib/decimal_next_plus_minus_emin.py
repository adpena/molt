"""Purpose: differential coverage for Decimal next_plus / next_minus near Emin.

Before the fix, next_plus/next_minus derived the step epsilon from the phantom
Etiny (1-prec)-prec+1 and the overflow boundary from emax = prec-1, so the
neighbours were wrong by ~6 orders of magnitude. CPython steps by 1e(Etiny-1)
where Etiny = Emin - prec + 1, and uses Etop = Emax - prec + 1 for infinities.
"""

from decimal import Decimal, Context, localcontext


def main():
    # Default context: Etiny = -1000026, so the smallest step is 1e-1000026.
    print("0 next_plus", Decimal("0").next_plus())
    print("0 next_minus", Decimal("0").next_minus())
    print("1 next_plus", Decimal("1").next_plus())
    print("1 next_minus", Decimal("1").next_minus())

    # Infinity neighbours use Etop with prec nines.
    print("inf next_minus", Decimal("Infinity").next_minus())
    print("inf next_plus", Decimal("Infinity").next_plus())
    print("-inf next_plus", Decimal("-Infinity").next_plus())
    print("-inf next_minus", Decimal("-Infinity").next_minus())

    # Custom small context exercises the Etiny/Etop derivation explicitly.
    ctx = Context(prec=3, Emin=-9, Emax=9, clamp=0)
    with localcontext(ctx):
        print("ctx Etiny", ctx.Etiny())
        print("ctx Etop", ctx.Etop())
        print("10 next_plus", Decimal("10").next_plus())
        print("10 next_minus", Decimal("10").next_minus())
        print("0 next_plus", Decimal("0").next_plus())
        print("0.10 next_minus", Decimal("0.10").next_minus())
        # Near the top of the representable range.
        print("999 next_plus", Decimal("999").next_plus())
        print("inf next_minus", Decimal("Infinity").next_minus())


if __name__ == "__main__":
    main()
