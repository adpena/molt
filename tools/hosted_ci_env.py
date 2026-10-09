#!/usr/bin/env python3
"""Reproduce a hosted GitHub Actions job's environment for a local checkout.

A hosted CI job exports process-wide state that a local run lacks:

* the hosted checkout custody contract: ``MOLT_CI_EPHEMERAL_CUSTODY_ROOT`` and
  the GitHub provenance fields that ``molt.dx`` verifies before it derives
  every artifact root from the runner's temp directory;
* the job's resource plan from ``tools/ci_resource_env.py``: RSS caps, Cargo
  jobs and pytest-xdist workers.

A test that passes locally can fail only under that state, and finding out
costs a CI round trip. This tool prints the same state for a local checkout,
so such failures reproduce in seconds::

    eval "$(python3 tools/hosted_ci_env.py --runner-temp /tmp/molt-runner)"
    uv run --python 3.12 python -m pytest -q tests/cli

The resource plan is computed for this host, as the setup action computes it
for the runner, so caps match CI in kind but not in value. The tool refuses to
print an environment that ``molt.dx.checkout_custody`` does not accept as
hosted custody. It writes only the runner temp directory and the event payload
inside it.
"""

from __future__ import annotations

import argparse
from collections.abc import Mapping
import json
import platform
from pathlib import Path
import shlex
import sys

ROOT = Path(__file__).resolve().parents[1]
_SRC_ROOT = ROOT / "src"
if str(_SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(_SRC_ROOT))

from molt import dx  # noqa: E402

if __package__:
    from . import ci_resource_env
else:  # pragma: no cover - direct script execution
    import ci_resource_env  # type: ignore

REPOSITORY = "adpena/molt"
_RUNNER_ARCH = {
    "amd64": "X64",
    "x86_64": "X64",
    "aarch64": "ARM64",
    "arm64": "ARM64",
    "x86": "X86",
    "i386": "X86",
    "i686": "X86",
}


def runner_os() -> str:
    if sys.platform == "win32":
        return "Windows"
    return "macOS" if sys.platform == "darwin" else "Linux"


def runner_arch() -> str:
    machine = platform.machine().lower()
    try:
        return _RUNNER_ARCH[machine]
    except KeyError:
        raise SystemExit(f"hosted_ci_env: no GitHub runner arch for {machine!r}")


def hosted_job_env(
    repo_root: Path,
    runner_temp: Path,
    *,
    sha: str,
    workflow: str = "ci.yml",
    event_name: str = "pull_request",
    ref: str = "refs/pull/1/merge",
    job: str = "python-unit",
    run_id: str = "1",
    run_attempt: str = "1",
    custody_dirname: str = "molt-custody",
) -> dict[str, str]:
    """The custody contract a hosted job exports for ``repo_root``.

    Creates ``runner_temp`` and writes the event payload into it; the custody
    root itself is left for the consumer to create, as on a runner.
    """

    runner_temp.mkdir(parents=True, exist_ok=True)
    event_path = runner_temp / "event.json"
    event_path.write_text(
        json.dumps({"repository": {"full_name": REPOSITORY}}), encoding="utf-8"
    )
    return {
        dx.GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV: str(runner_temp / custody_dirname),
        "GITHUB_ACTIONS": "true",
        "CI": "true",
        "GITHUB_REPOSITORY": REPOSITORY,
        "GITHUB_SERVER_URL": "https://github.com",
        "GITHUB_API_URL": "https://api.github.com",
        "GITHUB_WORKSPACE": str(repo_root.resolve()),
        "GITHUB_WORKFLOW_REF": f"{REPOSITORY}/.github/workflows/{workflow}@{ref}",
        "GITHUB_WORKFLOW_SHA": sha,
        "GITHUB_EVENT_PATH": str(event_path),
        "GITHUB_EVENT_NAME": event_name,
        "GITHUB_REF": ref,
        "GITHUB_SHA": sha,
        "GITHUB_RUN_ID": run_id,
        "GITHUB_RUN_ATTEMPT": run_attempt,
        "GITHUB_JOB": job,
        "RUNNER_TEMP": str(runner_temp.resolve()),
        "RUNNER_OS": runner_os(),
        "RUNNER_ARCH": runner_arch(),
    }


def resource_plan_env() -> dict[str, str]:
    """The resource variables the setup action exports, planned for this host."""

    lines = ci_resource_env.github_env_lines(ci_resource_env.plan_ci_resources())
    return dict(line.split("=", 1) for line in lines)


def render(env: Mapping[str, str], shell: str) -> str:
    if shell == "posix":
        return "".join(
            f"export {key}={shlex.quote(value)}\n" for key, value in env.items()
        )
    return "".join(
        "$env:{} = '{}'\n".format(key, value.replace("'", "''"))
        for key, value in env.items()
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--runner-temp",
        type=Path,
        required=True,
        help="directory that plays RUNNER_TEMP; must lie outside the checkout",
    )
    parser.add_argument("--checkout", type=Path, default=ROOT)
    parser.add_argument("--format", choices=("posix", "powershell"), default="posix")
    parser.add_argument(
        "--no-resource-plan",
        action="store_true",
        help="emit only the custody contract, without RSS caps and job counts",
    )
    args = parser.parse_args(argv)

    repo_root = args.checkout.resolve()
    runner_temp = args.runner_temp.expanduser().resolve()
    # The contract compares GITHUB_SHA with this same reader's result.
    sha = dx.git_checkout_head(repo_root)
    if sha is None:
        print(f"hosted_ci_env: {repo_root} has no git HEAD", file=sys.stderr)
        return 2
    env = hosted_job_env(repo_root, runner_temp, sha=sha)
    try:
        custody = dx.checkout_custody(repo_root, env)
    except dx.DxConfigError as exc:
        print(
            f"hosted_ci_env: molt.dx rejects the emulated job: {exc}", file=sys.stderr
        )
        return 2
    if custody.kind != "github-actions-ephemeral":
        print(
            f"hosted_ci_env: molt.dx resolved {custody.kind!r} custody, "
            "not the hosted contract",
            file=sys.stderr,
        )
        return 2
    if not args.no_resource_plan:
        env.update(resource_plan_env())
    sys.stdout.write(render(env, args.format))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
