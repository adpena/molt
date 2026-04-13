from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any

if __package__ in {None, ""}:
    REPO_ROOT = Path(__file__).resolve().parents[3]
    if str(REPO_ROOT) not in sys.path:
        sys.path.insert(0, str(REPO_ROOT))

from drivers.cloudflare.thin_adapter.verify import (
    run_wrangler_check,
    run_wrangler_dry_run,
    validate_bundle_contract,
)
from drivers.falcon.browser_webgpu.deploy import (
    DEFAULT_WRANGLER_CONFIG,
    discover_wrangler_config,
    materialize_deploy_bundle,
)


def verify_materialized_bundle(
    *,
    target_root: Path,
    weights_base_url: str | None,
    weights_root: Path | None,
    wrangler: str,
    wrangler_config: Path | None = None,
    bundle_root: Path | None = None,
    project_root: Path | None = None,
    env: dict[str, str] | None = None,
    verbose: bool = False,
    run_id: str | None = None,
) -> dict[str, Any]:
    config_path = discover_wrangler_config(wrangler_config or DEFAULT_WRANGLER_CONFIG)
    bundle = materialize_deploy_bundle(
        config_path=config_path,
        target_root=target_root.resolve(),
        weights_base_url=weights_base_url,
        weights_root=weights_root.resolve() if weights_root else None,
        bundle_root=bundle_root,
    )
    project_root = (project_root or REPO_ROOT).resolve()
    env_map = dict(os.environ if env is None else env)
    bundle_root_path = Path(bundle["bundle_root"])
    wrangler_config_path = Path(bundle["wrangler_config"])
    contract = validate_bundle_contract(
        bundle_root=bundle_root_path,
        wrangler_config=wrangler_config_path,
    )
    check = run_wrangler_check(
        wrangler=wrangler,
        bundle_root=bundle_root_path,
        wrangler_config=wrangler_config_path,
        project_root=project_root,
        env=env_map,
        verbose=verbose,
        run_id=run_id,
    )
    dry_run = run_wrangler_dry_run(
        wrangler=wrangler,
        bundle_root=bundle_root_path,
        wrangler_config=wrangler_config_path,
        project_root=project_root,
        env=env_map,
        verbose=verbose,
        run_id=run_id,
    )
    return {
        "target": "falcon.browser_webgpu",
        "bundle": bundle,
        "bundle_contract": contract,
        "wrangler_check": {
            "returncode": check.returncode,
            "stdout": check.stdout,
            "stderr": check.stderr,
        },
        "wrangler_dry_run": {
            "returncode": dry_run.returncode,
            "stdout": dry_run.stdout,
            "stderr": dry_run.stderr,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Materialize and verify the Falcon browser WebGPU Cloudflare bundle"
    )
    parser.add_argument("--target-root", type=Path, required=True)
    parser.add_argument("--weights-base-url", type=str, default=None)
    parser.add_argument("--weights-root", type=Path, default=None)
    parser.add_argument("--wrangler", type=str, default="wrangler")
    parser.add_argument("--wrangler-config", type=Path, default=None)
    parser.add_argument("--bundle-root", type=Path, default=None)
    parser.add_argument("--run-id", type=str, default=None)
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    payload = verify_materialized_bundle(
        target_root=args.target_root,
        weights_base_url=args.weights_base_url,
        weights_root=args.weights_root,
        wrangler=args.wrangler,
        wrangler_config=args.wrangler_config,
        bundle_root=args.bundle_root,
        run_id=args.run_id,
        verbose=args.verbose,
    )
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
