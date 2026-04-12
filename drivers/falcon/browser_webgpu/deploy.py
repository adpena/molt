from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path
from typing import Any

if __package__ in {None, ""}:
    REPO_ROOT = Path(__file__).resolve().parents[3]
    if str(REPO_ROOT) not in sys.path:
        sys.path.insert(0, str(REPO_ROOT))
else:
    REPO_ROOT = Path(__file__).resolve().parents[3]

from drivers._shared.artifacts import artifact_record, directory_records
from drivers._shared.cloudflare import extract_r2_bucket_names, load_jsonc


DRIVER_DIR = Path(__file__).resolve().parent
DEFAULT_WRANGLER_CONFIG = DRIVER_DIR / "wrangler.jsonc"
DEFAULT_ARTIFACT_SUBDIR = Path("dist") / "browser_split"
DEFAULT_BUNDLE_SUBDIR = Path("dist") / "cloudflare_browser_webgpu"
DEFAULT_WEIGHTS_SUBDIR = Path("weights")
DEFAULT_CONFIG_FILENAME = "config.json"
DEFAULT_TOKENIZER_FILENAME = "tokenizer.json"
DEFAULT_WEIGHTS_BUCKET = "falcon-ocr-weights"
DEFAULT_MANIFEST_ASSET_NAME = "driver-manifest.base.json"
DEFAULT_MANIFEST_ROUTE = "/driver-manifest.json"


def discover_wrangler_config(explicit: Path | None = None) -> Path:
    candidate = (explicit or DEFAULT_WRANGLER_CONFIG).expanduser().resolve()
    if not candidate.exists():
        raise FileNotFoundError(f"wrangler config not found: {candidate}")
    if not candidate.is_relative_to(DRIVER_DIR):
        raise ValueError(f"wrangler config must live under {DRIVER_DIR}")
    return candidate


def load_wrangler_config(path: Path) -> dict[str, Any]:
    return load_jsonc(path)


def _copy_file(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)


def _write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def build_deploy_surface(config_path: Path, target_root: Path) -> dict[str, Any]:
    config = load_wrangler_config(config_path)
    entrypoint = (config_path.parent / str(config.get("main", ""))).resolve()
    if not entrypoint.exists():
        raise FileNotFoundError(f"worker entrypoint not found: {entrypoint}")

    artifact_root = target_root / DEFAULT_ARTIFACT_SUBDIR
    app_wasm = artifact_root / "app.wasm"
    runtime_wasm = artifact_root / "molt_runtime.wasm"
    config_json = target_root / DEFAULT_CONFIG_FILENAME
    weights_dir = target_root / DEFAULT_WEIGHTS_SUBDIR
    tokenizer_json = weights_dir / DEFAULT_TOKENIZER_FILENAME
    if not app_wasm.exists():
        raise FileNotFoundError(f"missing Falcon app wasm: {app_wasm}")
    if not runtime_wasm.exists():
        raise FileNotFoundError(f"missing Falcon runtime wasm: {runtime_wasm}")
    if not config_json.exists():
        raise FileNotFoundError(f"missing Falcon config json: {config_json}")
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
            "config_json": artifact_record(
                kind="config_json",
                path=config_json,
                root=target_root,
            ),
            **(
                {
                    "tokenizer_json": artifact_record(
                        kind="tokenizer_json",
                        path=tokenizer_json,
                        root=weights_dir,
                    )
                }
                if tokenizer_json.exists()
                else {}
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
            "assets": config.get("assets", {}),
            "vars": config.get("vars", {}),
            "r2_buckets": config.get("r2_buckets", []),
            "observability": config.get("observability", {}),
        },
        "artifacts": {
            "app_wasm": str(app_wasm),
            "runtime_wasm": str(runtime_wasm),
            "config_json": str(config_json),
            "tokenizer_json": str(tokenizer_json) if tokenizer_json.exists() else None,
            "weights_dir": str(weights_dir),
        },
        "artifact_manifest": artifact_manifest,
    }


def _base_runtime_manifest(
    surface: dict[str, Any],
    *,
    weights_base_url: str | None,
) -> dict[str, Any]:
    immutable = surface["artifact_manifest"]["immutable"]
    return {
        "version": 1,
        "target": surface["target"],
        "artifacts": {
            "app_wasm": {
                "url": "/app.wasm",
                "sha256": immutable["app_wasm"]["sha256"],
                "size_bytes": immutable["app_wasm"]["size_bytes"],
            },
            "runtime_wasm": {
                "url": "/molt_runtime.wasm",
                "sha256": immutable["runtime_wasm"]["sha256"],
                "size_bytes": immutable["runtime_wasm"]["size_bytes"],
            },
            "config_json": {
                "url": "/config.json",
                "sha256": immutable["config_json"]["sha256"],
                "size_bytes": immutable["config_json"]["size_bytes"],
            },
            **(
                {
                    "tokenizer_json": {
                        "url": "/tokenizer.json",
                        "sha256": immutable["tokenizer_json"]["sha256"],
                        "size_bytes": immutable["tokenizer_json"]["size_bytes"],
                    }
                }
                if "tokenizer_json" in immutable
                else {}
            ),
            "browser_loader": {
                "url": "/browser.js",
                "sha256": immutable["browser_loader"]["sha256"],
                "size_bytes": immutable["browser_loader"]["size_bytes"],
            },
        },
        "weights": {
            "base_url": weights_base_url,
            "files": [
                {
                    "path": record["relative_path"],
                    "url": record["relative_path"],
                    "sha256": record["sha256"],
                    "size_bytes": record["size_bytes"],
                }
                for record in surface["artifact_manifest"]["weights"]
            ],
        },
        "exports": {
            "init": "main_molt__init",
            "ocrTokens": "main_molt__ocr_tokens",
        },
    }


def _materialized_wrangler_config(
    surface: dict[str, Any],
    *,
    weights_base_url: str | None,
) -> dict[str, Any]:
    cloudflare = surface["cloudflare"]
    vars_payload = dict(cloudflare.get("vars", {}))
    vars_payload.setdefault("DRIVER_TARGET", surface["target"])
    if weights_base_url is not None:
        vars_payload["WEIGHTS_BASE_URL"] = weights_base_url
    return {
        "name": cloudflare.get("name"),
        "compatibility_date": cloudflare.get("compatibility_date"),
        "main": "./drivers/falcon/browser_webgpu/worker.ts",
        "assets": {
            "directory": "./assets",
            "binding": "ASSETS",
            "run_worker_first": [DEFAULT_MANIFEST_ROUTE],
        },
        "vars": vars_payload,
        "r2_buckets": cloudflare.get("r2_buckets", []),
        "observability": cloudflare.get("observability", {}),
    }


def materialize_deploy_bundle(
    *,
    config_path: Path,
    target_root: Path,
    weights_base_url: str | None,
    bundle_root: Path | None = None,
) -> dict[str, Any]:
    if not weights_base_url:
        raise ValueError("weights_base_url is required for Cloudflare thin-adapter bundles")
    surface = build_deploy_surface(config_path=config_path, target_root=target_root)
    bundle_root = (bundle_root or (target_root / DEFAULT_BUNDLE_SUBDIR)).resolve()
    assets_root = bundle_root / "assets"
    falcon_worker_dst = bundle_root / "drivers" / "falcon" / "browser_webgpu" / "worker.ts"
    thin_worker_dst = bundle_root / "drivers" / "cloudflare" / "thin_adapter" / "worker.ts"
    bundle_root.mkdir(parents=True, exist_ok=True)
    assets_root.mkdir(parents=True, exist_ok=True)

    _copy_file(Path(surface["artifacts"]["app_wasm"]), assets_root / "app.wasm")
    _copy_file(Path(surface["artifacts"]["runtime_wasm"]), assets_root / "molt_runtime.wasm")
    _copy_file(Path(surface["artifacts"]["config_json"]), assets_root / "config.json")
    if surface["artifacts"]["tokenizer_json"]:
        _copy_file(Path(surface["artifacts"]["tokenizer_json"]), assets_root / "tokenizer.json")
    _copy_file(REPO_ROOT / "wasm" / "browser_host.js", assets_root / "browser_host.js")
    _copy_file(REPO_ROOT / "wasm" / "molt_vfs_browser.js", assets_root / "molt_vfs_browser.js")
    browser_loader_text = (DRIVER_DIR / "browser.js").read_text(encoding="utf-8").replace(
        'import { loadMoltWasm } from "../../../wasm/browser_host.js";',
        'import { loadMoltWasm } from "./browser_host.js";',
    )
    _write_text(assets_root / "browser.js", browser_loader_text)
    _copy_file(DRIVER_DIR / "worker.ts", falcon_worker_dst)
    _copy_file(
        REPO_ROOT / "drivers" / "cloudflare" / "thin_adapter" / "worker.ts",
        thin_worker_dst,
    )

    manifest_base = _base_runtime_manifest(surface, weights_base_url=weights_base_url)
    manifest_path = assets_root / DEFAULT_MANIFEST_ASSET_NAME
    manifest_path.write_text(json.dumps(manifest_base, indent=2) + "\n", encoding="utf-8")

    wrangler_config = _materialized_wrangler_config(
        surface,
        weights_base_url=weights_base_url,
    )
    wrangler_path = bundle_root / "wrangler.jsonc"
    wrangler_path.write_text(json.dumps(wrangler_config, indent=2) + "\n", encoding="utf-8")

    return {
        "target": surface["target"],
        "bundle_root": str(bundle_root),
        "wrangler_config": str(wrangler_path),
        "manifest_asset": str(manifest_path),
        "worker_entrypoint": str(falcon_worker_dst),
        "assets_root": str(assets_root),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Discover and materialize the Falcon browser WebGPU Cloudflare deploy surface"
    )
    parser.add_argument(
        "--target-root",
        type=Path,
        required=True,
        help="Falcon application root containing dist/browser_split, config.json, and weights/",
    )
    parser.add_argument(
        "--wrangler-config",
        type=Path,
        default=None,
        help="Optional target-local wrangler.jsonc override.",
    )
    parser.add_argument(
        "--weights-base-url",
        type=str,
        default=None,
        help="Optional public immutable base URL for Falcon weight blobs.",
    )
    parser.add_argument(
        "--bundle-root",
        type=Path,
        default=None,
        help="Optional output directory for the materialized Cloudflare deploy bundle.",
    )
    parser.add_argument(
        "--materialize-bundle",
        action="store_true",
        help="Materialize a Cloudflare-ready bundle instead of only printing the source surface.",
    )
    args = parser.parse_args()

    config_path = discover_wrangler_config(args.wrangler_config)
    if args.materialize_bundle:
        payload = materialize_deploy_bundle(
            config_path=config_path,
            target_root=args.target_root.resolve(),
            weights_base_url=args.weights_base_url,
            bundle_root=args.bundle_root,
        )
    else:
        payload = build_deploy_surface(config_path, args.target_root.resolve())
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
