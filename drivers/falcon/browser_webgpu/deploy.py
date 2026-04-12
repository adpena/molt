from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

if __package__ in {None, ""}:
    REPO_ROOT = Path(__file__).resolve().parents[3]
    if str(REPO_ROOT) not in sys.path:
        sys.path.insert(0, str(REPO_ROOT))

from drivers._shared.artifacts import artifact_record, directory_records
from drivers._shared.cloudflare import extract_r2_bucket_names, load_jsonc


DRIVER_DIR = Path(__file__).resolve().parent
DEFAULT_WRANGLER_CONFIG = DRIVER_DIR / "wrangler.jsonc"
DEFAULT_ARTIFACT_SUBDIR = Path("dist") / "browser_split"
DEFAULT_WEIGHTS_SUBDIR = Path("weights")
DEFAULT_WEIGHTS_BUCKET = "falcon-ocr-weights"


def discover_wrangler_config(explicit: Path | None = None) -> Path:
    candidate = (explicit or DEFAULT_WRANGLER_CONFIG).expanduser().resolve()
    if not candidate.exists():
        raise FileNotFoundError(f"wrangler config not found: {candidate}")
    if not candidate.is_relative_to(DRIVER_DIR):
        raise ValueError(f"wrangler config must live under {DRIVER_DIR}")
    return candidate


def load_wrangler_config(path: Path) -> dict[str, Any]:
    return load_jsonc(path)


def build_deploy_surface(config_path: Path, target_root: Path) -> dict[str, Any]:
    config = load_wrangler_config(config_path)
    entrypoint = (config_path.parent / str(config.get("main", ""))).resolve()
    if not entrypoint.exists():
        raise FileNotFoundError(f"worker entrypoint not found: {entrypoint}")

    artifact_root = target_root / DEFAULT_ARTIFACT_SUBDIR
    app_wasm = artifact_root / "app.wasm"
    runtime_wasm = artifact_root / "molt_runtime.wasm"
    if not app_wasm.exists():
        raise FileNotFoundError(f"missing Falcon app wasm: {app_wasm}")
    if not runtime_wasm.exists():
        raise FileNotFoundError(f"missing Falcon runtime wasm: {runtime_wasm}")
    weights_dir = target_root / DEFAULT_WEIGHTS_SUBDIR
    if not weights_dir.exists():
        raise FileNotFoundError(f"missing Falcon weights dir: {weights_dir}")

    r2_bucket_names = extract_r2_bucket_names(config)
    if DEFAULT_WEIGHTS_BUCKET not in r2_bucket_names:
        raise ValueError(
            f"{config_path} must bind the {DEFAULT_WEIGHTS_BUCKET!r} R2 bucket"
        )

    browser_loader = (DRIVER_DIR / "browser.js").resolve()
    if not browser_loader.exists():
        raise FileNotFoundError(f"browser loader not found: {browser_loader}")

    artifact_manifest = {
        "immutable": {
            "app_wasm": artifact_record(
                kind="wasm_module",
                path=app_wasm,
                root=target_root,
            ),
            "runtime_wasm": artifact_record(
                kind="wasm_runtime",
                path=runtime_wasm,
                root=target_root,
            ),
            "browser_loader": artifact_record(
                kind="browser_loader",
                path=browser_loader,
                root=DRIVER_DIR,
            ),
            "worker_entrypoint": artifact_record(
                kind="worker_entrypoint",
                path=entrypoint,
                root=DRIVER_DIR,
            ),
            "wrangler_config": artifact_record(
                kind="wrangler_config",
                path=config_path,
                root=DRIVER_DIR,
            ),
        },
        "weights": directory_records(kind="weights_blob", root=weights_dir),
    }

    return {
        "status": "manifest_ready",
        "target": "falcon.browser_webgpu",
        "target_root": str(target_root),
        "config_path": str(config_path),
        "worker_entrypoint": str(entrypoint),
        "cloudflare": {
            "name": config.get("name"),
            "compatibility_date": config.get("compatibility_date"),
            "main": config.get("main"),
            "r2_buckets": config.get("r2_buckets", []),
            "observability": config.get("observability", {}),
        },
        "artifacts": {
            "app_wasm": str(app_wasm),
            "runtime_wasm": str(runtime_wasm),
            "weights_dir": str(weights_dir),
        },
        "artifact_manifest": artifact_manifest,
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Discover the Falcon browser WebGPU Cloudflare deploy surface"
    )
    parser.add_argument(
        "--target-root",
        type=Path,
        required=True,
        help="Falcon application root containing dist/browser_split and weights/",
    )
    parser.add_argument(
        "--wrangler-config",
        type=Path,
        default=None,
        help="Optional target-local wrangler.jsonc override.",
    )
    args = parser.parse_args()

    config_path = discover_wrangler_config(args.wrangler_config)
    surface = build_deploy_surface(config_path, args.target_root.resolve())
    print(json.dumps(surface, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
