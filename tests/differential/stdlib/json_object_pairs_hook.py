"""Purpose: differential coverage for json object_pairs_hook ordering."""

import json

text = '{"b": 2, "a": 1}'

pairs = json.loads(text, object_pairs_hook=lambda pairs: pairs)
print("pairs", pairs)
