"""Proof command, environment, Cargo, and toolchain admission policy."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Mapping

from tools.proof_queue_pkg import (
    command_admission,
    custody,
    execution_environment,
    state,
)


def _proof_env_policy_error(env_overrides: dict[str, str]) -> str | None:
    semantic_error = execution_environment.environment_override_policy_error(
        env_overrides
    )
    if semantic_error is not None:
        return f"proof queue refuses invalid environment override: {semantic_error}"
    try:
        custody._proof_queue_memory_limits(env_overrides)
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
        error = wasm_toolchain_module.rust_target_readiness_error(
            target, root=repo_root
        )
        if error is not None:
            return [error]
    return None


def _command_basename(command: str) -> str:
    return Path(command).name.lower()


def _normalized_cargo_args(cargo_args: list[str]) -> list[str]:
    args = list(cargo_args)
    if args[:1] == ["--"]:
        args = args[1:]
    if args and _command_basename(args[0]) in {"cargo", "cargo.exe"}:
        args = args[1:]
    return args


def _cold_single_lib_test_policy_error(cargo_args: list[str]) -> str | None:
    invocation = command_admission.parse_cargo_invocation(
        ["cargo", *_normalized_cargo_args(cargo_args)]
    )
    if invocation.proof_kind != "test-execution" or "--lib" not in invocation.flags:
        return None
    filters = invocation.positionals
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
    secret_error = execution_environment.command_secret_policy_error(command)
    if secret_error is not None:
        return f"proof queue refuses secret-bearing command: {secret_error}"
    try:
        envelope = command_admission.envelope_for_command(command)
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
    python = envelope.get("python")
    if not isinstance(python, Mapping):
        return "proof queue refuses `uv run` without a typed Python authority"
    prefix = python.get("prefix")
    if not isinstance(prefix, list):
        return "proof queue refuses `uv run` without a typed launch prefix"
    prefix = [str(value) for value in prefix]
    missing = []
    if "--active" not in prefix:
        missing.append("--active")
    if command_admission._uv_option_values(prefix, "--project") != ["."]:
        missing.append("--project .")
    if command_admission._uv_option_values(prefix, "--python") != ["3.12"]:
        missing.append("--python 3.12")
    if "--no-sync" not in prefix:
        missing.append("--no-sync")
    if "--no-config" not in prefix:
        missing.append("--no-config")
    if not missing:
        return None
    return (
        "proof queue refuses `uv run` commands without the active project "
        "interpreter contract; missing "
        + ", ".join(missing)
        + ". Use `uv run --active --project . --python 3.12 --no-sync --no-config ...`."
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
    seen: dict[str, str] = {}
    for pair in pairs:
        name, value = _parse_env_pair(pair)
        folded = name.casefold()
        if folded in seen:
            raise SystemExit(
                "duplicate environment override names are forbidden: "
                f"{seen[folded]!r}, {name!r}"
            )
        seen[folded] = name
        env[name] = value
    return env


def _env_table(raw: object, *, error: str) -> dict[str, str]:
    if not isinstance(raw, Mapping):
        raise SystemExit(error)
    result: dict[str, str] = {}
    for key, value in raw.items():
        if not isinstance(key, str) or not isinstance(value, str):
            raise SystemExit(error)
        result[key] = value
    return result


def _env_overrides_from_spec(raw: object) -> dict[str, str]:
    if raw is None:
        return {}
    if isinstance(raw, Mapping):
        return _env_table(
            raw, error="proof env table must contain string keys and string values"
        )
    if isinstance(raw, list):
        pairs: list[str] = []
        for item in raw:
            if not isinstance(item, str):
                raise SystemExit(
                    "proof env must be a table of strings or a list of NAME=VALUE strings"
                )
            pairs.append(item)
        return _env_overrides_from_pairs(pairs)
    raise SystemExit(
        "proof env must be a table of strings or a list of NAME=VALUE strings"
    )


def _named_spec_locked_env(logical_id: str, raw_locked: object) -> dict[str, str]:
    error = (
        f"named proof {logical_id!r} has invalid locked_env authority; "
        "expected a list of non-empty environment variable names"
    )
    if not isinstance(raw_locked, (list, tuple)):
        raise SystemExit(error)
    locked_by_casefold: dict[str, str] = {}
    for name in raw_locked:
        if not isinstance(name, str) or not name:
            raise SystemExit(error)
        folded = name.casefold()
        if folded in locked_by_casefold:
            raise SystemExit(
                f"named proof {logical_id!r} has duplicate locked_env authority"
            )
        locked_by_casefold[folded] = name
    return locked_by_casefold


def _named_spec_user_env_overrides(
    logical_id: str, locked_by_casefold: Mapping[str, str], user_pairs: list[str]
) -> dict[str, str]:
    """Admit user diagnostics against the validated named environment authority."""

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
            "environment custody: " + ", ".join(conflicts)
        )
    return user_overrides


def _named_spec_env_overrides(
    spec: Mapping[str, object], user_pairs: list[str]
) -> dict[str, str]:
    """Merge admitted diagnostics with a named proof's canonical environment."""
    logical_id = str(spec.get("logical_id") or "named-proof")
    locked_by_casefold = _named_spec_locked_env(logical_id, spec.get("locked_env", ()))
    user_overrides = _named_spec_user_env_overrides(
        logical_id, locked_by_casefold, user_pairs
    )

    raw_defaults = spec.get("env_overrides", {})
    env_overrides = _env_table(
        raw_defaults,
        error=f"named proof {logical_id!r} has invalid env_overrides authority",
    )
    defaults_by_casefold = {name.casefold() for name in env_overrides}
    missing_locked = sorted(
        name
        for folded, name in locked_by_casefold.items()
        if folded not in defaults_by_casefold
    )
    if missing_locked:
        raise SystemExit(
            f"named proof {logical_id!r} has locked environment names without "
            "canonical launch values: " + ", ".join(missing_locked)
        )
    env_overrides.update(user_overrides)
    policy_error = _proof_env_policy_error(env_overrides)
    if policy_error is not None:
        raise SystemExit(f"named proof {logical_id!r} {policy_error}")
    return env_overrides


def _uv_active_python_command(*args: str) -> list[str]:
    command = [
        "uv",
        "run",
        "--active",
        "--project",
        ".",
        "--python",
        "3.12",
        "--no-sync",
        "--no-config",
    ]
    command.append("python")
    command.extend(args)
    return command


def _cargo_package_for_contention(cargo_args: list[str]) -> str:
    invocation = command_admission.parse_cargo_invocation(
        ["cargo", *_normalized_cargo_args(cargo_args)]
    )
    for name, value in invocation.option_values:
        if name == "--package":
            return state._slug(value)
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
    )


def _load_json_mapping(path: Path) -> Mapping[str, object] | None:
    try:
        loaded = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return None
    return loaded if isinstance(loaded, Mapping) else None
