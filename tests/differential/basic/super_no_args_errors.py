"""Purpose: differential coverage for super() no-args errors outside methods."""

if __name__ == "__main__":
    try:
        super()
        print("super", "missed")
    except Exception as exc:
        print("super", type(exc).__name__)
