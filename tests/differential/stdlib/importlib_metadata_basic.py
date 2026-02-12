"""Purpose: differential coverage for importlib metadata basic."""

import importlib.metadata as md


try:
    dist = md.distribution("pip")
    print(dist.metadata["Name"])
except Exception as exc:
    print(type(exc).__name__)
