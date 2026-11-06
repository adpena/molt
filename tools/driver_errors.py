"""Exit statuses and failure types shared by driver adapters and low execution owners."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from pathlib import Path
    from molt.process_environment import ProcessEnvironment

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2


class DriverError(Exception):
    """A loud, fatal driver condition. Carries an exit code (default FAIL)."""

    def __init__(
        self,
        message: str,
        code: int = EXIT_FAIL,
        *,
        maintenance_selection: tuple[Path, ProcessEnvironment] | None = None,
    ):
        super().__init__(message)
        self.code = code
        # Only a completed command attaches its captured selection. The CLI
        # publishes this failure before scheduling its independent maintenance.
        self.maintenance_selection = maintenance_selection


# The landing queue attributes a failure to a candidate only when the
# candidate caused it; these name the outcomes it must tell apart.
class CandidateFailure(DriverError):
    """The candidate failed a check that its own commits decide."""


class GateFailure(CandidateFailure):
    """A selected gate failed on the candidate.

    ``clean_closure`` is true only when every other gate of the block either
    passed or was cancelled with a closed tree and no runner fault: then the
    landing may rerun the failed gate alone once (molt_dev_gates).
    """

    clean_closure: bool = False
    # The failed gate's display form; the landing reruns exactly that gate.
    shown: tuple[str, ...] = ()


class RebaseConflict(DriverError):
    """The candidate does not apply to the current upstream."""


class PublicationRace(DriverError):
    """Upstream moved after the candidate's upstream was selected."""
