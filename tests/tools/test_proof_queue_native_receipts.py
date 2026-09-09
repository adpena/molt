"""Real supervisor execution/verification; never selected by the pure unit lane."""

from functools import lru_cache, partial
from pathlib import Path
import subprocess
import sys

import pytest

from tests.process_guard_common import run_custody_subject_process
from tests.proof_queue_custody_test_support import (
    assert_execution_context_rejects_substitutions,
    publish_receipt_custody,
)
from tools.proof_queue_pkg import command_admission

pytestmark = pytest.mark.slow


@lru_cache(maxsize=1)
def _native_supervisor_binary() -> Path:
    build = (
        Path(command_admission.__file__).resolve().parents[1]
        / "proof_supervisor"
        / "build.py"
    )
    completed = run_custody_subject_process(
        [sys.executable, str(build), "--release"],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    )
    return Path(completed.stdout.splitlines()[-1]).resolve(strict=True)


def test_native_execution_context_rehashes_nonce_custody_and_transcript_artifacts(
    tmp_path: Path,
) -> None:
    """Prove the same binding assertions through the actual native verifier."""

    def execute(command: list[str]) -> None:
        run_custody_subject_process(command, check=True)

    factory = partial(
        publish_receipt_custody,
        supervisor_binary=_native_supervisor_binary(),
        execute_supervisor=execute,
    )
    assert_execution_context_rejects_substitutions(tmp_path, factory)
