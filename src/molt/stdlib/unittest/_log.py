"""unittest._log — logging capture support for Molt.

Provides ``_AssertLogsContext`` for ``TestCase.assertLogs()`` /
``assertNoLogs()``.  Requires ``logging`` module support.
"""

from __future__ import annotations

from typing import Any

__all__ = ["_AssertLogsContext", "_AssertNoLogsContext"]


class _CapturingHandler:
    """Minimal logging handler that captures records into a list."""

    def __init__(self) -> None:
        self.records: list[Any] = []

    def emit(self, record: Any) -> None:
        self.records.append(record)

    # Satisfy logging.Handler interface minimally
    def handle(self, record: Any) -> None:
        self.emit(record)

    def setLevel(self, level: Any) -> None:
        self._level = level

    def getEffectiveLevel(self) -> int:
        return getattr(self, "_level", 0)


class _AssertLogsContext:
    """Context manager returned by ``TestCase.assertLogs()``."""

    LOGGING_FORMAT = "%(levelname)s:%(name)s:%(message)s"

    def __init__(self, test_case: Any, logger_name: str | None, level: int) -> None:
        self._case = test_case
        self._logger_name = logger_name
        self._level = level
        self.records: list[Any] = []
        self.output: list[str] = []

    def __enter__(self) -> "_AssertLogsContext":
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> bool:
        if exc_type is not None:
            return False
        if not self.records:
            self._case.fail(
                f"No logs of level {self._level} or higher triggered on "
                f"{self._logger_name!r}"
            )
        return False


class _AssertNoLogsContext:
    """Context manager returned by ``TestCase.assertNoLogs()``."""

    def __init__(self, test_case: Any, logger_name: str | None, level: int) -> None:
        self._case = test_case
        self._logger_name = logger_name
        self._level = level
        self.records: list[Any] = []

    def __enter__(self) -> "_AssertNoLogsContext":
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> bool:
        if exc_type is not None:
            return False
        if self.records:
            self._case.fail(f"Unexpected logs triggered on {self._logger_name!r}")
        return False
