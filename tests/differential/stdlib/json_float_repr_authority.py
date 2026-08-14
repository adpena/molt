"""Purpose: json.dumps shares the canonical CPython float repr authority."""

import json


values = [1e16, 1e-5, 1e100, 5e-324, 137839762462415.62, -0.0]
print([json.dumps(value) for value in values])
