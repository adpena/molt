"""Explicit development provisioning of the sealed consumer's pinned archives.

This command may download archive data. The consumer remains offline/cache-only;
no payload is installed or executed here, and no Docker image is fetched.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from molt.tool_releases import provision_archive
from . import execution_root
from .release_model import ROOT, target_by_id


def provision(
    *, target_id: str, downloads: Path, source_root: Path = ROOT
) -> list[dict[str, Any]]:
    target = target_by_id(target_id)
    if target.platform != "linux":
        raise ValueError(
            f"sealed standalone replay has no admitted {target.platform}/{target.arch} filesystem adapter; archive provisioning cannot substitute host replay"
        )
    providers = execution_root.archive_inputs(arch=target.arch, source_root=source_root)
    for provider in providers:
        provision_archive(
            url=provider["url"],
            size=provider["size"],
            sha256=provider["sha256"],
            downloads=downloads,
        )
    # Exercise the exact same pinned descriptor/payload boundary as the consumer,
    # before any build or guest launch. Cache presence alone cannot grant success.
    payloads, admitted = execution_root.support_payloads(
        downloads, arch=target.arch, source_root=source_root
    )
    if admitted != providers:
        raise ValueError(
            "execution-root provider authority changed during provisioning"
        )
    execution_root.audit_native_payload_closure(
        payloads, arch=target.arch, executable_paths=["bin/node"]
    )
    return providers


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--execution-archive-cache", type=Path, required=True)
    args = parser.parse_args()
    providers = provision(target_id=args.target, downloads=args.execution_archive_cache)
    print(json.dumps({"target": args.target, "archives": providers}, sort_keys=True))


if __name__ == "__main__":
    main()
