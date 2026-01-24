"""Purpose: differential coverage for bool len exceptions."""


class BoolErr:
    def __bool__(self):
        raise RuntimeError("boom")


try:
    bool(BoolErr())
except Exception as exc:
    print(type(exc).__name__)


class LenErr:
    def __len__(self):
        raise RuntimeError("len")


try:
    bool(LenErr())
except Exception as exc:
    print(type(exc).__name__)
