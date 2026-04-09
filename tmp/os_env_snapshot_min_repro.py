def _MOLT_OS_ENVIRON():
    return {"A": "B"}


def _MOLT_ENV_SNAPSHOT():
    return {"A": "B"}


def f():
    if callable(_MOLT_OS_ENVIRON):
        raw = _MOLT_OS_ENVIRON()
        if isinstance(raw, dict):
            out = {}
            for key, value in raw.items():
                if not isinstance(key, str) or not isinstance(value, str):
                    raise RuntimeError("os env snapshot intrinsic returned invalid value")
                out[key] = value
            return out
        if isinstance(raw, (list, tuple)):
            out2 = {}
            it = iter(raw)
            for k in it:
                v = next(it)
                if not isinstance(k, str) or not isinstance(v, str):
                    raise RuntimeError("os env snapshot intrinsic returned invalid value")
                out2[k] = v
            return out2
    raw_legacy = _MOLT_ENV_SNAPSHOT()
    if not isinstance(raw_legacy, dict):
        raise RuntimeError("os env snapshot intrinsic returned invalid value")
    out3 = {}
    for key, value in raw_legacy.items():
        if not isinstance(key, str) or not isinstance(value, str):
            raise RuntimeError("os env snapshot intrinsic returned invalid value")
        out3[key] = value
    return out3


print(type(f()).__name__)
