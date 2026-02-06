print("hasattr", hasattr({}, "__get__"))
try:
    print("getattr", getattr({}, "__get__"))
except Exception as exc:
    print("getattr_exc", type(exc).__name__, exc)
