"""Purpose: differential coverage for __exit__ raising exceptions."""

class Manager:
    def __enter__(self):
        return "enter"

    def __exit__(self, exc_type, exc, tb):
        raise RuntimeError("exit")


if __name__ == "__main__":
    try:
        with Manager():
            raise KeyError("inner")
    except Exception as exc:
        print("type", type(exc).__name__)
        print("context", type(exc.__context__).__name__)
