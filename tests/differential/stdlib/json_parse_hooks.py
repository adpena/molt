"""Purpose: differential coverage for json parse_* hooks."""

import json
from decimal import Decimal


def main():
    calls = []

    def parse_constant(value):
        calls.append(value)
        return f"CONST:{value}"

    payload = '{"a": 1.5, "b": 2, "c": NaN, "d": Infinity, "e": -Infinity}'
    data = json.loads(
        payload,
        parse_float=Decimal,
        parse_int=str,
        parse_constant=parse_constant,
    )
    print("types", type(data["a"]).__name__, type(data["b"]).__name__)
    print("values", data["a"], data["b"], data["c"], data["d"], data["e"])
    print("constants", calls)


if __name__ == "__main__":
    main()
