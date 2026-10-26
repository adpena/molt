"""The original native resource witness inputs, shared by preparation and execution.

Expected results stay in the end-to-end tests. No runtime tier or feature list
is selected here: each real CLI build derives them from these Python sources.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class ResourceWitness:
    source: str
    environment: dict[str, str]


def child_resource_witness(python: str) -> ResourceWitness:
    keys = [
        "MOLT_RESOURCE_MAX_MEMORY",
        "MOLT_RESOURCE_MAX_DURATION_MS",
        "MOLT_RESOURCE_MAX_ALLOCATIONS",
        "MOLT_RESOURCE_MAX_RECURSION_DEPTH",
        "MOLT_RESOURCE_MAX_OPERATION_RESULT",
        "MOLT_RESOURCE_MAX_POW_RESULT",
        "MOLT_RESOURCE_MAX_REPEAT_RESULT",
        "MOLT_RESOURCE_MAX_SHIFT_RESULT",
        "MOLT_RESOURCE_MAX_STRING_RESULT",
    ]
    initial = dict(
        zip(
            keys,
            ["64M", "30000", "1000000", "500", "4096", "2048", "2048", "2048", "2048"],
        )
    )
    initial.update(MOLT_CAPABILITY_TIER="full", MOLT_CHILD_RLIMIT_GB="1")
    child = (
        "import os; print(','.join(os.environ.get(k, 'missing') for k in "
        + repr(keys)
        + ")); print(os.environ.get('APPLICATION_VALUE', 'missing'))"
    )
    source = f"""
import os
import subprocess
keys = {keys!r}
for key in keys:
    os.putenv(key, "999999999")
    os.environ[key] = "999999999"
os.putenv("MOLT_CHILD_RLIMIT_GB", "0")
os.environ["APPLICATION_VALUE"] = "dynamic"
for extended in (False, True):
    for requested in (None, {{"MOLT_RESOURCE_MAX_MEMORY": "32M", "MOLT_RESOURCE_MAX_OPERATION_RESULT": "1024", "APPLICATION_VALUE": "replacement"}}):
        result = subprocess.run([{python!r}, "-c", {child!r}], env=requested, capture_output=True, text=True, start_new_session=extended)
        assert result.returncode == 0, result.stderr
        print(result.stdout, end="")
"""
    return ResourceWitness(source, initial)


def feedback_destination_witness(canary: Path) -> ResourceWitness:
    source = f"""
import os
os.putenv("MOLT_RUNTIME_FEEDBACK", "1")
os.putenv("MOLT_RUNTIME_FEEDBACK_FILE", {str(canary)!r})
os.environ["MOLT_RUNTIME_FEEDBACK"] = "1"
os.environ["MOLT_RUNTIME_FEEDBACK_FILE"] = {str(canary)!r}
print("completed")
"""
    return ResourceWitness(
        source,
        {
            "MOLT_CAPABILITY_TIER": "none",
            "MOLT_CAPABILITIES": "env.write",
            "MOLT_PROFILE": "1",
            "MOLT_RUNTIME_FEEDBACK": "0",
            "MOLT_RUNTIME_FEEDBACK_FILE": str(canary),
        },
    )
