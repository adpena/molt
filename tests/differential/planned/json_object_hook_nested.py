"""Purpose: differential coverage for json object_hook ordering."""

import json


def main():
    calls = []

    def hook(obj):
        calls.append(tuple(sorted(obj.keys())))
        return {"_keys": sorted(obj.keys()), "_obj": obj}

    data = json.loads('{"outer": {"inner": 1}, "x": 2}', object_hook=hook)
    print("calls", calls)
    print("outer_keys", data["_keys"])
    print("inner_keys", data["_obj"]["outer"]["_keys"])


if __name__ == "__main__":
    main()
