"""Purpose: differential coverage for class no init args."""


class NoInit:
    pass


NoInit()
print("no-args ok")
try:
    NoInit(1)
except Exception as exc:
    print("with-arg", type(exc).__name__)
