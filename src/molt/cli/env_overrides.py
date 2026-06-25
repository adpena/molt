from __future__ import annotations

from collections.abc import Iterator, Mapping
from contextlib import contextmanager
import os


@contextmanager
def temporary_env_overrides(overrides: Mapping[str, str]) -> Iterator[None]:
    previous = {name: os.environ.get(name) for name in overrides}
    try:
        for name, value in overrides.items():
            os.environ[name] = value
        yield
    finally:
        for name, old_value in previous.items():
            if old_value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = old_value
