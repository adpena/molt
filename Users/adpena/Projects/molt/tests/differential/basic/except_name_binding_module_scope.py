try:
    raise ValueError("negative")
except ValueError as e:
    print(type(e).__name__, str(e))
