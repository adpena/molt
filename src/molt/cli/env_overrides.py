"""The CLI's one sanctioned writer of the process environment.

A value the CLI resolves (a flag, a configuration entry, a backend choice)
never travels through ``os.environ``: it flows as a typed parameter, and a child
process receives it in an environment mapping built for that child.

This helper serves a narrower case. An in-process stand-in for a separate
``molt build`` process must see that process's complete ambient environment,
because a build reads its ambient inputs (module roots, capability and cache
settings, and hundreds more) from ``os.environ``. Two callers do this: the batch
build server, which applies each request's client-supplied environment, and a
wrapper build, which predicts its child's module graph under the child's own
mapping. Each overlay covers one serial operation and is restored afterwards. It
is process-global and not safe while another thread builds. Removing it needs
an explicit environment parameter through every ambient reader of a build.
``tests/cli/test_cli_environment_custody.py`` keeps every other CLI write out.
"""

from __future__ import annotations

from collections.abc import Generator, Mapping
from contextlib import contextmanager
import os


@contextmanager
def temporary_env_overrides(overrides: Mapping[str, str]) -> Generator[None]:
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
