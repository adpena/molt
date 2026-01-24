"""Purpose: differential coverage for __getattribute__ fallback to __getattr__."""

class Demo:
    def __getattribute__(self, name):
        if name == "block":
            raise AttributeError("blocked")
        return super().__getattribute__(name)

    def __getattr__(self, name):
        return f"fallback:{name}"


if __name__ == "__main__":
    demo = Demo()
    print("fallback", demo.missing)
    try:
        demo.block
    except Exception as exc:
        print("block", type(exc).__name__)
