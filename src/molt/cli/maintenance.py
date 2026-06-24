from __future__ import annotations

import contextlib
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any, Sequence

from molt.cli.config_resolution import (
    _resolve_build_config,
    _resolve_capabilities_config,
    _resolve_command_config,
)


def _cli_module() -> Any:
    import importlib

    return importlib.import_module("molt.cli")


def _find_molt_root(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._find_molt_root(*args, **kwargs)


def _require_molt_root(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._require_molt_root(*args, **kwargs)


def _fail(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._fail(*args, **kwargs)


def _default_molt_home() -> Path:
    return _cli_module()._default_molt_home()


def _default_molt_bin() -> Path:
    return _cli_module()._default_molt_bin()


def _default_molt_cache() -> Path:
    return _cli_module()._default_molt_cache()


def _format_capabilities_input(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._format_capabilities_input(*args, **kwargs)


def _json_payload(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._json_payload(*args, **kwargs)


def _emit_json(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._emit_json(*args, **kwargs)


def _load_artifact_cleanup_module(root: Path) -> Any:
    tool_path = root / "tools" / "artifact_cleanup.py"
    if not tool_path.is_file():
        raise FileNotFoundError(f"missing canonical cleanup tool: {tool_path}")
    spec = importlib.util.spec_from_file_location(
        "_molt_repo_artifact_cleanup",
        tool_path,
    )
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load canonical cleanup tool: {tool_path}")
    module = importlib.util.module_from_spec(spec)
    root_text = str(root)
    inserted_root = False
    if root_text not in sys.path:
        sys.path.insert(0, root_text)
        inserted_root = True
    try:
        spec.loader.exec_module(module)
    finally:
        if inserted_root:
            with contextlib.suppress(ValueError):
                sys.path.remove(root_text)
    return module


def clean(
    json_output: bool = False,
    verbose: bool = False,
    apply: bool = False,
    kill_processes: bool = False,
    extra_paths: Sequence[str] | None = None,
    list_paths: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
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


def show_config(
    config_root: Path,
    config: dict[str, Any],
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
    data: dict[str, Any] = {
        "root": str(config_root),
        "sources": {
            "molt_toml": str(molt_toml) if molt_toml.exists() else None,
            "pyproject": str(pyproject) if pyproject.exists() else None,
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
