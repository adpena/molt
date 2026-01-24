"""Purpose: differential coverage for exception notes."""

if __name__ == "__main__":
    try:
        raise ValueError("boom")
    except ValueError as exc:
        exc.add_note("note-1")
        exc.add_note("note-2")
        print("notes", exc.__notes__)
