"""Purpose: differential coverage for importlib invalidate caches."""

import importlib


print(importlib.invalidate_caches() is None)
