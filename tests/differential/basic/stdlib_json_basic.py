# MOLT_ENV: MOLT_CODEC=json

import json


def main():
    payload = {"a": 1, "b": [2, 3], "c": True, "d": None}
    text = json.dumps(payload, sort_keys=True)
    print(text)
    data = json.loads(text)
    print(data["a"], data["b"][0], data["c"], data["d"])


if __name__ == "__main__":
    main()
