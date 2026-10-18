from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path, PurePosixPath
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[3]
_REPO_ROOT = Path(__file__).resolve().parents[3]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from drivers._shared.artifacts import artifact_record, directory_records  # noqa: E402
from drivers._shared.cloudflare import (  # noqa: E402
    extract_r2_bucket_names,
    load_jsonc,
)
from tools.gen_browser_asset_graph import AssetSource, scan_resolved_sources  # noqa: E402
from molt.exact_json import capture_exact  # noqa: E402
from molt.wasm_artifact import WASM_RUNTIME_MANIFEST_NAME  # noqa: E402
from molt.browser_asset_closure import (  # noqa: E402
    BROWSER_HOST_ENTRY_ASSETS,
    canonical_wasm_loader_asset_bytes,
    wasm_loader_asset_closure,
)


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


def validate_browser_asset_records(root: Path, records: list[dict[str, Any]]) -> None:
    """Validate retained loader references using the generator's checked scanner."""
    sources = {}
    for record in records:
        name = record["relative_path"]
        is_javascript = PurePosixPath(name).suffix in {".js", ".mjs"}
        content = (root / name).read_bytes() if is_javascript else b""
        if is_javascript and (
            len(content) != record["size_bytes"]
            or hashlib.sha256(content).hexdigest() != record["sha256"]
        ):
            raise ValueError(f"Falcon browser asset bytes changed before scan: {name}")
        sources[name] = AssetSource(
            path=name,
            role="browser",
            source_type="module" if is_javascript else "data",
            authority="selected Falcon deployment",
            content=content,
        )
    scan_resolved_sources(sources)


def execution_artifact_records(root: Path) -> tuple[str, list[dict[str, Any]]]:
    """Admit the build's execution manifest and its complete declared file closure."""
    root = root.resolve()
    manifest_path = root / WASM_RUNTIME_MANIFEST_NAME
    identity, manifest = capture_exact(
        manifest_path, max_bytes=16 * 1024 * 1024, label="Falcon execution manifest"
    )
    if (
        not isinstance(manifest, dict)
        or manifest.get("version") != 2
        or manifest.get("mode") not in ("linked", "split-runtime")
    ):
        raise ValueError(
            "Falcon requires a version 2 linked or split-runtime execution manifest"
        )
    mode = manifest["mode"]
    modules = manifest.get("modules")
    assets = manifest.get("assets", {})
    required_modules = {"linked"} if mode == "linked" else {"app", "runtime"}
    if (
        not isinstance(modules, dict)
        or set(modules) != required_modules
        or not isinstance(assets, dict)
    ):
        raise ValueError(
            f"Falcon execution manifest requires modules {sorted(required_modules)} and assets"
        )
    records = {
        WASM_RUNTIME_MANIFEST_NAME: {
            "kind": "execution_manifest",
            "path": str(manifest_path),
            "relative_path": WASM_RUNTIME_MANIFEST_NAME,
            "size_bytes": identity.size,
            "sha256": identity.sha256,
        }
    }
    asset_paths = set()
    for section, descriptors in (("modules", modules), ("assets", assets)):
        for role, descriptor in descriptors.items():
            label = f"{section}.{role}"
            if not isinstance(descriptor, dict):
                raise ValueError(f"Falcon execution manifest malformed {label}")
            name = descriptor.get("path")
            relative = PurePosixPath(name) if isinstance(name, str) else None
            if (
                relative is None
                or not name
                or relative.is_absolute()
                or ".." in relative.parts
                or "\\" in name
                or ":" in name
                or relative.as_posix() != name
            ):
                raise ValueError(f"Falcon execution manifest invalid path for {label}")
            path = root.joinpath(*relative.parts).resolve()
            if not path.is_relative_to(root) or not path.is_file():
                raise ValueError(
                    f"Falcon execution manifest missing or escaped {label}: {name}"
                )
            record = artifact_record(kind=section, path=path, root=root)
            record["relative_path"] = name
            if (
                type(descriptor.get("size")) is not int
                or descriptor["size"] != record["size_bytes"]
                or descriptor.get("sha256") != record["sha256"]
            ):
                raise ValueError(
                    f"Falcon execution manifest bytes mismatch for {label}: {name}"
                )
            if name in records:
                raise ValueError(f"Falcon execution manifest duplicate file: {name}")
            records[name] = record
            if section == "assets":
                asset_paths.add(name)
    if mode == "split-runtime" and "browser_host.js" not in asset_paths:
        raise ValueError(
            "Falcon split execution manifest missing browser_host.js asset"
        )
    if mode == "split-runtime":
        validate_browser_asset_records(root, list(records.values()))
    return mode, list(records.values())


def discover_falcon_config_json(target_root: Path, *, weights_root: Path) -> Path:
    root_config = target_root / DEFAULT_CONFIG_FILENAME
    if root_config.exists():
        return root_config
    weights_config = weights_root / DEFAULT_CONFIG_FILENAME
    if weights_config.exists():
        return weights_config
    raise FileNotFoundError(
        f"missing Falcon config json: expected {root_config} or {weights_config}"
    )


def build_deploy_surface(
    config_path: Path,
    target_root: Path,
    *,
    weights_root: Path | None = None,
) -> dict[str, Any]:
    config = load_wrangler_config(config_path)
    entrypoint = (config_path.parent / str(config.get("main", ""))).resolve()
    if not entrypoint.exists():
        raise FileNotFoundError(f"worker entrypoint not found: {entrypoint}")

    artifact_root = target_root / DEFAULT_ARTIFACT_SUBDIR
    mode, execution_artifacts = execution_artifact_records(artifact_root)
    hosting_assets = []
    if mode == "linked":
        # Linked CLI output declares its executable, not a browser hosting kit.
        # Select the kit once here; receivers verify these retained byte records.
        declared_files = {item["relative_path"]: item for item in execution_artifacts}
        for name in wasm_loader_asset_closure(
            REPO_ROOT / "wasm", BROWSER_HOST_ENTRY_ASSETS
        ):
            path = REPO_ROOT / "wasm" / name
            payload = canonical_wasm_loader_asset_bytes(path)
            record = {
                "kind": "browser_host",
                "path": str(path),
                "relative_path": name,
                "size_bytes": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
            declared = declared_files.get(name)
            if declared is not None and (
                declared["sha256"],
                declared["size_bytes"],
            ) != (record["sha256"], record["size_bytes"]):
                raise ValueError(
                    f"Linked hosting asset conflicts with declared artifact: {name}"
                )
            hosting_assets.append(record)
            del payload
    weights_dir = (
        (weights_root or (target_root / DEFAULT_WEIGHTS_SUBDIR)).expanduser().resolve()
    )
    config_json = discover_falcon_config_json(target_root, weights_root=weights_dir)
    tokenizer_json = weights_dir / DEFAULT_TOKENIZER_FILENAME
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
    config_root = target_root.resolve()
    if not config_json.is_relative_to(config_root):
        config_root = weights_dir

    artifact_manifest = {
        "immutable": {
            "execution_manifest": execution_artifacts[0],
            "config_json": artifact_record(
                kind="config_json",
                path=config_json,
                root=config_root,
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
        "execution_artifacts": execution_artifacts,
        "hosting_assets": hosting_assets,
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
            "execution_manifest": str(artifact_root / WASM_RUNTIME_MANIFEST_NAME),
            "config_json": str(config_json),
            "tokenizer_json": str(tokenizer_json) if tokenizer_json.exists() else None,
            "weights_dir": str(weights_dir),
        },
        "artifact_manifest": artifact_manifest,
    }


def _base_driver_manifest(
    surface: dict[str, Any],
    *,
    weights_base_url: str | None,
) -> dict[str, Any]:
    immutable = surface["artifact_manifest"]["immutable"]
    return {
        "version": 1,
        "target": surface["target"],
        "artifacts": {
            "execution_manifest": {
                "url": "/runtime/manifest.json",
                "sha256": immutable["execution_manifest"]["sha256"],
                "size_bytes": immutable["execution_manifest"]["size_bytes"],
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
        **(
            {
                "hosting_assets": [
                    {
                        "relative_path": item["relative_path"],
                        "size_bytes": item["size_bytes"],
                        "sha256": item["sha256"],
                    }
                    for item in surface["artifact_manifest"]["hosting_assets"]
                ]
            }
            if surface["artifact_manifest"]["hosting_assets"]
            else {}
        ),
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
    weights_root: Path | None = None,
    bundle_root: Path | None = None,
) -> dict[str, Any]:
    surface = build_deploy_surface(
        config_path=config_path,
        target_root=target_root,
        weights_root=weights_root,
    )
    bundle_root = (bundle_root or (target_root / DEFAULT_BUNDLE_SUBDIR)).resolve()
    assets_root = bundle_root / "assets"
    falcon_worker_dst = (
        bundle_root / "drivers" / "falcon" / "browser_webgpu" / "worker.ts"
    )
    thin_worker_dst = (
        bundle_root / "drivers" / "cloudflare" / "thin_adapter" / "worker.ts"
    )
    bundle_root.mkdir(parents=True, exist_ok=True)
    assets_root.mkdir(parents=True, exist_ok=True)

    runtime_root = assets_root / "runtime"
    for record in surface["artifact_manifest"]["execution_artifacts"]:
        destination = runtime_root.joinpath(
            *PurePosixPath(record["relative_path"]).parts
        )
        _copy_file(Path(record["path"]), destination)
        if (
            destination.stat().st_size != record["size_bytes"]
            or artifact_record(kind=record["kind"], path=destination)["sha256"]
            != record["sha256"]
        ):
            raise ValueError(
                f"Falcon execution artifact changed during publication: {record['relative_path']}"
            )
    for record in surface["artifact_manifest"]["hosting_assets"]:
        payload = canonical_wasm_loader_asset_bytes(Path(record["path"]))
        if (
            len(payload) != record["size_bytes"]
            or hashlib.sha256(payload).hexdigest() != record["sha256"]
        ):
            raise ValueError(
                f"Falcon hosting asset changed during publication: {record['relative_path']}"
            )
        destination = runtime_root.joinpath(
            *PurePosixPath(record["relative_path"]).parts
        )
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(payload)
        del payload
    _copy_file(Path(surface["artifacts"]["config_json"]), assets_root / "config.json")
    if surface["artifacts"]["tokenizer_json"]:
        _copy_file(
            Path(surface["artifacts"]["tokenizer_json"]), assets_root / "tokenizer.json"
        )
    browser_loader_text = (
        (DRIVER_DIR / "browser.js")
        .read_text(encoding="utf-8")
        .replace(
            'import { loadMoltWasm } from "../../../wasm/browser_host.js";',
            'import { loadMoltWasm } from "./runtime/browser_host.js";',
        )
    )
    _write_text(assets_root / "browser.js", browser_loader_text)
    surface["artifact_manifest"]["immutable"]["browser_loader"] = artifact_record(
        kind="browser_loader", path=assets_root / "browser.js", root=assets_root
    )
    _copy_file(DRIVER_DIR / "worker.ts", falcon_worker_dst)
    _copy_file(
        REPO_ROOT / "drivers" / "cloudflare" / "thin_adapter" / "worker.ts",
        thin_worker_dst,
    )

    manifest_base = _base_driver_manifest(surface, weights_base_url=weights_base_url)
    manifest_path = assets_root / DEFAULT_MANIFEST_ASSET_NAME
    manifest_path.write_text(
        json.dumps(manifest_base, indent=2) + "\n", encoding="utf-8"
    )

    wrangler_config = _materialized_wrangler_config(
        surface,
        weights_base_url=weights_base_url,
    )
    wrangler_path = bundle_root / "wrangler.jsonc"
    wrangler_path.write_text(
        json.dumps(wrangler_config, indent=2) + "\n", encoding="utf-8"
    )

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
        help=(
            "Falcon application root containing dist/browser_split and weights/ "
            "(config may live at the root or under weights/config.json)."
        ),
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
        "--weights-root",
        type=Path,
        default=None,
        help="Optional alternate local weights root used for manifest weight metadata.",
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
            weights_root=args.weights_root.resolve() if args.weights_root else None,
            bundle_root=args.bundle_root,
        )
    else:
        payload = build_deploy_surface(
            config_path,
            args.target_root.resolve(),
            weights_root=args.weights_root.resolve() if args.weights_root else None,
        )
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
