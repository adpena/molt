"""Purpose: differential coverage for json error cases and edge options."""

import json


def show(loads_input):
    try:
        json.loads(loads_input)
        print("ok", loads_input)
    except Exception as exc:
        print("err", type(exc).__name__)


show("{bad")
show("[1,]")
show('"\\ud800"')

try:
    json.dumps(float("nan"), allow_nan=False)
except Exception as exc:
    print("dump_err", type(exc).__name__)
