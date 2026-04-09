def f():
    raw = {"A": "B"}
    if isinstance(raw, dict):
        out = {}
        for key, value in raw.items():
            if not isinstance(key, str) or not isinstance(value, str):
                raise RuntimeError("bad")
            out[key] = value
        return out
    return {}


print(type(f()).__name__)
