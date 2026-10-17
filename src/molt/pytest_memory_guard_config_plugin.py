from __future__ import annotations

from collections.abc import Generator

import pytest

from molt import pytest_memory_guard_bootstrap as _bootstrap
from molt.pytest_memory_guard_bootstrap import (  # noqa: F401
    pytest_configure,
    pytest_runtest_call,
    pytest_runtest_logfinish,
    pytest_runtest_logstart,
    pytest_runtest_setup,
    pytest_runtest_teardown,
)


@pytest.hookimpl(wrapper=True, tryfirst=True)
def pytest_load_initial_conftests(
    early_config: pytest.Config, parser: pytest.Parser, args: list[str]
) -> Generator[None]:
    # The guard hand-off execs this process, and the exec'd guard inherits
    # fds 1 and 2. Hand off outside pytest's capture wrapper, which points
    # them at temporary files: an in-process pytest.main() would print nothing.
    _bootstrap.pytest_load_initial_conftests(early_config, parser, args)
    return (yield)
