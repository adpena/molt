"""Purpose: differential coverage for subprocess env mapping inputs."""

from collections.abc import Mapping
import subprocess
import sys


class EnvMapping(Mapping):
    def __init__(self, data):
        self._data = dict(data)

    def __getitem__(self, key):
        return self._data[key]

    def __iter__(self):
        return iter(self._data)

    def __len__(self):
        return len(self._data)


def _cmd():
    return [
        sys.executable,
        "-c",
        "import os; print(os.environ.get('MOLT_SUBPROCESS_MAPPING', 'missing'))",
    ]


def main():
    env = EnvMapping({"MOLT_SUBPROCESS_MAPPING": "ok"})

    run_result = subprocess.run(
        _cmd(),
        env=env,
        capture_output=True,
        text=True,
        check=True,
    )
    print("run_env", run_result.stdout.strip())

    with subprocess.Popen(
        _cmd(),
        env=env,
        stdout=subprocess.PIPE,
        text=True,
    ) as proc:
        popen_out = proc.stdout.read().strip()
    print("popen_env", popen_out)


if __name__ == "__main__":
    main()
