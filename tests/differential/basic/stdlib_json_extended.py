# MOLT_ENV: MOLT_CODEC=json

import json


class Reader:
    def __init__(self, text):
        self._text = text

    def read(self):
        return self._text


def main():
    payload = {"b": 1, "a": 2}
    print(json.dumps(payload, sort_keys=True, separators=(",", ":")))

    print(json.dumps([1, 2], indent=2))
    print(json.dumps({"x": 1}, indent="--", sort_keys=True))

    snowman = "snow:\u2603"
    print(json.dumps({"snow": snowman}, ensure_ascii=False, sort_keys=True))

    def default(_obj):
        return {"__type__": "X"}

    print(json.dumps(object(), default=default, sort_keys=True))

    def parse_float(text):
        return f"f:{text}"

    def parse_int(text):
        return f"i:{text}"

    def parse_constant(text):
        return f"c:{text}"

    print(
        json.loads(
            "[1.5, 2, NaN, -Infinity]",
            parse_float=parse_float,
            parse_int=parse_int,
            parse_constant=parse_constant,
        )
    )

    def pairs_hook(pairs):
        return [f"{key}:{value}" for key, value in pairs]

    print(json.loads('{"b":1,"a":2}', object_pairs_hook=pairs_hook))

    def object_hook(obj):
        return {"sum": obj["a"] + obj["b"]}

    print(json.loads('{"a":1,"b":2}', object_hook=object_hook))

    reader = Reader('{"a": 1}')
    print(json.load(reader))

    print(json.dumps({1: "one", (1, 2): "tuple"}, skipkeys=True, sort_keys=True))

    try:
        json.dumps(float("nan"), allow_nan=False)
    except Exception as exc:  # pragma: no cover - parity check only
        print(type(exc).__name__)


if __name__ == "__main__":
    main()
