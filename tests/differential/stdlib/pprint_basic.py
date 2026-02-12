"""Purpose: differential coverage for pprint basic."""

import pprint


data = {"a": [1, 2, 3], "b": {"x": 1, "y": 2}}
print(pprint.pformat(data, width=20))
