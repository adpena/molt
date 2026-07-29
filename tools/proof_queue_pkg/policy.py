"""Proof command, environment, Cargo, and toolchain admission policy."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Mapping

from tools.proof_queue_pkg import command_envelope, custody, state


def _proof_env_policy_error(env_overrides: dict[str, str]) -> str | None:
    try:
        custody._proof_queue_memory_guard_poll_sec(env_overrides)
    except ValueError as exc:
        return f"proof queue refuses invalid environment override: {exc}"
    return None



def _load_wasm_toolchain():
    from molt.cli import wasm_toolchain

    return wasm_toolchain



def _required_rust_targets_for_resource(
    resource_family: str, *, repo_root: Path, wasm_toolchain_module=None
) -> tuple[str, ...]:
    if resource_family in state.WASM_RESOURCE_FAMILIES:
        wasm_toolchain_module = wasm_toolchain_module or _load_wasm_toolchain()
        return wasm_toolchain_module.rust_toolchain_contract(
            repo_root
        ).required_wasm_targets
    return ()



def _ensure_run_toolchain_preflight(
    *,
    repo_root: Path,
    resource_family: str,
) -> list[str] | None:
    warnings: list[str] = []
    wasm_toolchain_module = None
    try:
        if resource_family in state.WASM_RESOURCE_FAMILIES:
            wasm_toolchain_module = _load_wasm_toolchain()
        required_targets = _required_rust_targets_for_resource(
            resource_family,
            repo_root=repo_root,
            wasm_toolchain_module=wasm_toolchain_module,
        )
    except ImportError as exc:
        return [f"failed to import WASM toolchain contract: {exc}"]
    except Exception as exc:
        contract_error = (
            getattr(wasm_toolchain_module, "RustToolchainContractError", None)
            if wasm_toolchain_module is not None
            else None
        )
        if contract_error is not None and isinstance(exc, contract_error):
            return [str(exc)]
        raise
    if wasm_toolchain_module is None:
        return None
    for target in required_targets:
        if not wasm_toolchain_module.ensure_rustup_target(
            target, warnings, root=repo_root
        ):
            if not warnings:
                warnings.append(f"failed to ensure Rust target {target}")
            return warnings
    return None



def _command_basename(command: str) -> str:
    return Path(command).name.lower()



def _has_option(command: list[str], option: str, value: str | None = None) -> bool:
    for index, arg in enumerate(command):
        if arg == option:
            return value is None or (
                index + 1 < len(command) and command[index + 1] == value
            )
        if value is not None and arg == f"{option}={value}":
            return True
    return False



_CARGO_OPTIONS_WITH_VALUES = frozenset(
    {
        "-p",
        "--package",
        "--manifest-path",
        "--target",
        "--target-dir",
        "--features",
        "--profile",
        "--jobs",
        "--config",
        "--message-format",
        "--color",
        "--bin",
        "--example",
        "--test",
        "--bench",
    }
)



def _normalized_cargo_args(cargo_args: list[str]) -> list[str]:
    args = list(cargo_args)
    if args[:1] == ["--"]:
        args = args[1:]
    if args and _command_basename(args[0]) in {"cargo", "cargo.exe"}:
        args = args[1:]
    return args



def _cargo_arg_has_flag(cargo_args: list[str], flag: str) -> bool:
    return any(arg == flag for arg in cargo_args)



def _cargo_test_filters(cargo_args: list[str]) -> list[str]:
    args = _normalized_cargo_args(cargo_args)
    if args[:1] != ["test"]:
        return []
    filters: list[str] = []
    skip_value = False
    for arg in args[1:]:
        if arg == "--":
            break
        if skip_value:
            skip_value = False
            continue
        if arg in _CARGO_OPTIONS_WITH_VALUES:
            skip_value = True
            continue
        if any(
            arg.startswith(f"{option}=")
            for option in _CARGO_OPTIONS_WITH_VALUES
            if option.startswith("--")
        ):
            continue
        if arg.startswith("-"):
            continue
        filters.append(arg)
    return filters



def _cold_single_lib_test_policy_error(cargo_args: list[str]) -> str | None:
    args = _normalized_cargo_args(cargo_args)
    if args[:1] != ["test"] or not _cargo_arg_has_flag(args, "--lib"):
        return None
    filters = _cargo_test_filters(args)
    if len(filters) != 1:
        return None
    return (
        "proof queue refuses cold-prone single-test Cargo proofs "
        f"({filters[0]!r} under --lib). Batch the relevant crate shard in one "
        "compile, warm the target dir with cargo check before proving, or "
        "resubmit with --allow-warm-single-test only after verifying the target "
        "dir is already warm and recording that in --note."
    )



def _proof_command_policy_error(command: list[str]) -> str | None:
    if not command:
        return None
    try:
        command_envelope.envelope_for_command(command)
    except ValueError as exc:
        return f"proof queue refuses an untyped command envelope: {exc}"
    basename = _command_basename(command[0])
    if basename in {"cargo", "cargo.exe"}:
        return (
            "proof queue refuses raw `cargo` commands; use "
            "`tools/proof_queue.py cargo ... -- <cargo-args>` so the queue owns "
            "the uv, guarded_exec, contention, timeout, and log envelope."
        )
    if len(command) < 2:
        return None
    if basename != "uv.exe" and basename != "uv":
        return None
    if command[1] != "run":
        return None
    missing = []
    if not _has_option(command, "--active"):
        missing.append("--active")
    if not _has_option(command, "--project", "."):
        missing.append("--project .")
    if not _has_option(command, "--python", "3.12"):
        missing.append("--python 3.12")
    if not missing:
        return None
    return (
        "proof queue refuses `uv run` commands without the active project "
        "interpreter contract; missing "
        + ", ".join(missing)
        + ". Use `uv run --active --project . --python 3.12 ...`."
    )



def _parse_env_pair(pair: str) -> tuple[str, str]:
    if "=" not in pair:
        raise SystemExit(f"env override {pair!r} must be NAME=VALUE")
    name, value = pair.split("=", 1)
    if not name:
        raise SystemExit("env override name must not be empty")
    return name, value



def _env_overrides_from_pairs(pairs: list[str]) -> dict[str, str]:
    env: dict[str, str] = {}
    for pair in pairs:
        name, value = _parse_env_pair(pair)
        env[name] = value
    return env



def _env_overrides_from_spec(raw: object) -> dict[str, str]:
    if raw is None:
        return {}
    if isinstance(raw, dict):
        if not all(
            isinstance(key, str) and isinstance(value, str)
            for key, value in raw.items()
        ):
            raise SystemExit(
                "proof env table must contain string keys and string values"
            )
        return dict(raw)
    if isinstance(raw, list) and all(isinstance(item, str) for item in raw):
        return _env_overrides_from_pairs(list(raw))
    raise SystemExit(
        "proof env must be a table of strings or a list of NAME=VALUE strings"
    )



def _named_spec_user_env_overrides(
    logical_id: str, raw_locked: object, user_pairs: list[str]
) -> dict[str, str]:
    """Admit user diagnostics without weakening a named proof's custody."""
    if not isinstance(raw_locked, (list, tuple)) or not all(
        isinstance(name, str) and name for name in raw_locked
    ):
        raise SystemExit(
            f"named proof {logical_id!r} has invalid locked_env authority; "
            "expected a list of non-empty environment variable names"
        )
    locked_by_casefold = {name.casefold(): name for name in raw_locked}
    if len(locked_by_casefold) != len(raw_locked):
        raise SystemExit(
            f"named proof {logical_id!r} has duplicate locked_env authority"
        )

    user_overrides = _env_overrides_from_pairs(user_pairs)
    conflicts = sorted(
        {
            locked_by_casefold[name.casefold()]
            for name in user_overrides
            if name.casefold() in locked_by_casefold
        }
    )
    if conflicts:
        raise SystemExit(
            f"named proof {logical_id!r} rejects --env overrides for locked "
            "environment custody: "
            + ", ".join(conflicts)
        )
    return user_overrides



def _named_spec_env_overrides(
    spec: Mapping[str, object], user_pairs: list[str]
) -> dict[str, str]:
    """Merge admitted diagnostics with a named proof's canonical environment."""
    logical_id = str(spec.get("logical_id") or "named-proof")
    user_overrides = _named_spec_user_env_overrides(
        logical_id, spec.get("locked_env", ()), user_pairs
    )

    raw_defaults = spec.get("env_overrides", {})
    if not isinstance(raw_defaults, Mapping) or not all(
        isinstance(name, str) and isinstance(value, str)
        for name, value in raw_defaults.items()
    ):
        raise SystemExit(
            f"named proof {logical_id!r} has invalid env_overrides authority"
        )
    env_overrides = dict(raw_defaults)
    locked_names = spec.get("locked_env", ())
    assert isinstance(locked_names, (list, tuple))
    defaults_by_casefold = {name.casefold() for name in env_overrides}
    missing_locked = sorted(
        name for name in locked_names if name.casefold() not in defaults_by_casefold
    )
    if missing_locked:
        raise SystemExit(
            f"named proof {logical_id!r} has locked environment names without "
            "canonical launch values: " + ", ".join(missing_locked)
        )
    env_overrides.update(user_overrides)
    return env_overrides



def _uv_active_python_command(
    *args: str,
    with_packages: list[str] | None = None,
    no_sync: bool = False,
) -> list[str]:
    command = ["uv", "run", "--active", "--project", ".", "--python", "3.12"]
    if no_sync:
        command.append("--no-sync")
    for package in with_packages or []:
        command.extend(["--with", package])
    command.append("python")
    command.extend(args)
    return command



def _cargo_package_for_contention(cargo_args: list[str]) -> str:
    for index, arg in enumerate(cargo_args):
        if arg in {"-p", "--package"} and index + 1 < len(cargo_args):
            return state._slug(cargo_args[index + 1])
        if arg.startswith("--package="):
            return state._slug(arg.split("=", 1)[1])
    return "workspace"



def _canonical_cargo_proof_command(cargo_args: list[str]) -> list[str]:
    args = _normalized_cargo_args(cargo_args)
    if not args:
        raise SystemExit("cargo proof command is empty")
    return _uv_active_python_command(
        "tools/guarded_exec.py",
        "--prefix",
        "MOLT_TEST_SUITE",
        "--",
        "cargo",
        *args,
        no_sync=True,
    )



def _load_json_mapping(path: Path) -> Mapping[str, object] | None:
    try:
        loaded = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return None
    return loaded if isinstance(loaded, Mapping) else None
