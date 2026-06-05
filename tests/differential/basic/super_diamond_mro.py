"""Purpose: diamond-inheritance super() must traverse the instance's REAL C3
MRO, not the lexically-defining class's first base.

Base <- Left, Base <- Right, Final(Left, Right).  Final's C3 MRO is
[Final, Left, Right, Base, object].  When Final().who() dispatches to
Left.who(), the super().who() inside Left MUST resolve to the next class
after Left in *Final's* MRO -- which is Right, NOT Base.  A frontend that
folds super() lexically to Left's own first base (Base) silently produces
the WRONG answer ("Left->Base" instead of "Left->Right->Base").

Output must be byte-identical to CPython 3.14.
"""


class Base:
    def who(self) -> str:
        return "Base"


class Left(Base):
    def who(self) -> str:
        return "Left->" + super().who()


class Right(Base):
    def who(self) -> str:
        return "Right->" + super().who()


class Final(Left, Right):
    def who(self) -> str:
        return "Final->" + super().who()


def main() -> None:
    # Final instance: full chain must be Final->Left->Right->Base.
    print("final", Final().who())
    # Left instance directly: its super is Base (Left's own MRO).
    print("left", Left().who())
    # Right instance directly: its super is Base.
    print("right", Right().who())
    # MRO introspection must match CPython's C3 linearization exactly.
    print("mro", [c.__name__ for c in Final.__mro__])
    # super() bound at the Left level on a Final instance traverses Right.
    f = Final()
    print("super_left", super(Left, f).who())
    # super() bound at the Final level on a Final instance traverses Left.
    print("super_final", super(Final, f).who())


if __name__ == "__main__":
    main()
