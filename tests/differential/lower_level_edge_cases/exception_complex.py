"""Purpose: differential coverage for exception complex."""


def section(name):
    print(f"--- {name} ---")


section("Raise from")
try:
    try:
        raise ValueError("Original error")
    except ValueError as e:
        raise RuntimeError("New error") from e
except RuntimeError as e:
    print(f"Caught: {type(e).__name__}: {e}")
    print(f"Cause: {type(e.__cause__).__name__}: {e.__cause__}")
    print(f"Context: {type(e.__context__).__name__}: {e.__context__}")

section("Raise from None")
try:
    try:
        raise ValueError("Original error")
    except ValueError:
        raise RuntimeError("New error") from None
except RuntimeError as e:
    print(f"Caught: {type(e).__name__}")
    print(f"Cause: {e.__cause__}")
    # Context is still suppressed but might exist depending on impl
    # Python suppresses display, but the object might differ
    print(f"Suppress Context: {e.__suppress_context__}")

section("Try/Except/Else/Finally Flow")


def flow_test(trigger_error):
    print(f"Testing trigger={trigger_error}")
    try:
        print("  try block")
        if trigger_error:
            raise ValueError("boom")
    except ValueError:
        print("  except block")
    else:
        print("  else block")
    finally:
        print("  finally block")


flow_test(False)
flow_test(True)
