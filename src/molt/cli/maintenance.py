from __future__ import annotations

import contextlib
import importlib.util
import json
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

from molt.source_root import compiler_source_root
from molt.cli.config_resolution import (
    _resolve_build_config,
    _resolve_capabilities_config,
    _resolve_command_config,
    resolve_stdlib_profile,
)
from molt.capability_policy import format_capability_input as _format_capabilities_input
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import _require_molt_root
from molt.default_paths import (
    _default_molt_bin,
    _default_molt_cache,
    _default_molt_home,
)


def _load_artifact_cleanup_module(root: Path) -> Any:
    """This checkout's `tools.artifact_cleanup`, under its one import identity."""
    tool_path = root / "tools" / "artifact_cleanup.py"
    if not tool_path.is_file():
        raise FileNotFoundError(f"missing canonical cleanup tool: {tool_path}")
    root_text = str(root)
    inserted_root = False
    if root_text not in sys.path:
        sys.path.insert(0, root_text)
        inserted_root = True
    try:
        module = importlib.import_module("tools.artifact_cleanup")
    finally:
        if inserted_root:
            with contextlib.suppress(ValueError):
                sys.path.remove(root_text)
    loaded = Path(getattr(module, "__file__", "") or "").resolve()
    if loaded != tool_path.resolve():
        raise ImportError(
            f"tools.artifact_cleanup is loaded from {loaded}, not this checkout's "
            f"{tool_path}"
        )
    return module


def clean(
    json_output: bool = False,
    verbose: bool = False,
    apply: bool = False,
    kill_processes: bool = False,
    extra_paths: Sequence[str] | None = None,
    list_paths: bool = False,
) -> int:
    root = compiler_source_root()
    root_error = _require_molt_root(root, json_output, "clean")
    if root_error is not None:
        return root_error
    assert root is not None
    try:
        artifact_cleanup = _load_artifact_cleanup_module(root)
    except Exception as exc:
        return _fail(str(exc), json_output, command="clean")

    argv = ["--repo-root", str(root)]
    if apply:
        argv.append("--apply")
    if kill_processes:
        argv.append("--kill-processes")
    if list_paths:
        argv.append("--list-paths")
    if json_output:
        argv.append("--json")
    if verbose:
        argv.append("--verbose")
    for pathspec in extra_paths or ():
        argv.extend(["--extra-path", pathspec])
    return int(artifact_cleanup.main(argv))


def cache_prune(
    json_output: bool,
    *,
    cache_dir: str | None,
    max_gb: float | None,
    max_age_days: int | None,
    dry_run: bool,
) -> int:
    """Apply the cache budget now: the documented, unthrottled user entry point."""
    import os

    from molt.cache_eviction import (
        _default_cache_root,
        _prune,
        prune_report_lines,
        resolve_budget,
    )

    root = (
        Path(cache_dir).expanduser().resolve() if cache_dir else _default_cache_root()
    )
    gb, days = resolve_budget(max_gb, max_age_days, os.environ)
    try:
        result = _prune(
            root, max_bytes=int(gb * 1024**3), max_age_days=days, dry_run=dry_run
        )
    except (OSError, ValueError, RuntimeError) as exc:
        return _fail(str(exc), json_output, command="cache prune")
    failed = result["status"] == "completed" and bool(result["entries_failed"])
    if json_output:
        data = {
            "cache_root": str(root),
            "max_gb": gb,
            "max_age_days": days,
            "dry_run": dry_run,
            **{
                name: value
                for name, value in result.items()
                if name not in {"protected_state", "legacy_native_state"}
            },
        }
        _emit_json(
            _json_payload("cache prune", "error" if failed else "ok", data=data),
            json_output=True,
        )
    else:
        for line in prune_report_lines(
            root, result, max_gb=gb, max_age_days=days, dry_run=dry_run
        ):
            print(line)
    return 1 if failed else 0


def show_config(
    config_root: Path,
    config: Mapping[str, Any],
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    molt_toml = config_root / "molt.toml"
    pyproject = config_root / "pyproject.toml"
    build_cfg = _resolve_build_config(config)
    run_cfg = _resolve_command_config(config, "run")
    compare_cfg = _resolve_command_config(config, "compare")
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    extension_cfg = _resolve_command_config(config, "extension")
    publish_cfg = _resolve_command_config(config, "publish")
    caps_cfg = _resolve_capabilities_config(config)
    stdlib_profile_value, stdlib_profile_source = resolve_stdlib_profile(
        flag=None,
        build_cfg=build_cfg,
    )
    data: dict[str, Any] = {
        "root": str(config_root),
        "sources": {
            "molt_toml": str(molt_toml) if molt_toml.exists() else None,
            "pyproject": str(pyproject) if pyproject.exists() else None,
        },
        "resolved": {
            "stdlib_profile": {
                "value": stdlib_profile_value,
                "source": stdlib_profile_source,
            },
        },
        "build": build_cfg,
        "run": run_cfg,
        "compare": compare_cfg,
        "test": test_cfg,
        "diff": diff_cfg,
        "extension": extension_cfg,
        "publish": publish_cfg,
        "capabilities": caps_cfg,
        "paths": {
            "molt_home": str(_default_molt_home()),
            "molt_bin": str(_default_molt_bin()),
            "molt_cache": str(_default_molt_cache()),
            "build_root": str(_default_molt_home() / "build"),
        },
    }
    if json_output:
        data["config"] = config
        payload = _json_payload("config", "ok", data=data)
        _emit_json(payload, json_output=True)
        return 0
    print(f"Config root: {config_root}")
    if data["sources"]["molt_toml"] or data["sources"]["pyproject"]:
        print("Sources:")
        if data["sources"]["molt_toml"]:
            print(f"- {data['sources']['molt_toml']}")
        if data["sources"]["pyproject"]:
            print(f"- {data['sources']['pyproject']}")
    print("Paths:")
    for key, value in data["paths"].items():
        print(f"- {key}: {value}")
    print("Resolved:")
    print(f"- stdlib_profile: {stdlib_profile_value} (source: {stdlib_profile_source})")
    if build_cfg:
        print("Build defaults:")
        for key in sorted(build_cfg):
            print(f"- {key}: {build_cfg[key]}")
    else:
        print("Build defaults: none")
    if run_cfg:
        print("Run defaults:")
        for key in sorted(run_cfg):
            print(f"- {key}: {run_cfg[key]}")
    else:
        print("Run defaults: none")
    if compare_cfg:
        print("Compare defaults:")
        for key in sorted(compare_cfg):
            print(f"- {key}: {compare_cfg[key]}")
    else:
        print("Compare defaults: none")
    if test_cfg:
        print("Test defaults:")
        for key in sorted(test_cfg):
            print(f"- {key}: {test_cfg[key]}")
    else:
        print("Test defaults: none")
    if diff_cfg:
        print("Diff defaults:")
        for key in sorted(diff_cfg):
            print(f"- {key}: {diff_cfg[key]}")
    else:
        print("Diff defaults: none")
    if extension_cfg:
        print("Extension defaults:")
        for key in sorted(extension_cfg):
            print(f"- {key}: {extension_cfg[key]}")
    else:
        print("Extension defaults: none")
    if publish_cfg:
        print("Publish defaults:")
        for key in sorted(publish_cfg):
            print(f"- {key}: {publish_cfg[key]}")
    else:
        print("Publish defaults: none")
    if caps_cfg is not None:
        print(f"Capabilities: {_format_capabilities_input(caps_cfg)}")
    else:
        print("Capabilities: none")
    if verbose:
        print("Merged config:")
        print(json.dumps(config, indent=2))
    return 0
