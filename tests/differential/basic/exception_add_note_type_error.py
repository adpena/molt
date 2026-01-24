"""Purpose: differential coverage for add_note type errors."""

if __name__ == "__main__":
    try:
        raise ValueError("boom")
    except ValueError as exc:
        try:
            exc.add_note(123)
            print("type", "missed")
        except Exception as err:
            print("type", type(err).__name__)
