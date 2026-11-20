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
    # Argument Clinic texts from 3.14 name None by value; 3.12 and 3.13 quote
    # the type name.
    for note in (None, 17, b"bytes", ["list"]):
        try:
            ValueError().add_note(note)
        except TypeError as err:
            print("note", type(note).__name__, str(err))
