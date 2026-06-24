import tomllib
from dataclasses import replace
from pathlib import Path
from typing import Any

from bench_friends_types import (
    RUNNER_NAME_RE,
    SUPPORTED_RUNNER_ROLES,
    SUPPORTED_SEMANTIC_MODES,
    RunnerSpec,
    SuiteSpec,
)

def _load_manifest(path: Path) -> tuple[dict[str, Any], list[SuiteSpec]]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    schema_version = int(data.get("schema_version", 1))
    defaults = data.get("defaults", {})
    suites_raw = data.get("suite", [])
    if not isinstance(suites_raw, list):
        raise ValueError("manifest `suite` must be an array of tables")
    suites = [_parse_suite(raw, defaults) for raw in suites_raw]
    return {"schema_version": schema_version}, suites


def _parse_suite(raw: dict[str, Any], defaults: dict[str, Any]) -> SuiteSpec:
    suite_id = str(raw.get("id", "")).strip()
    if not suite_id:
        raise ValueError("suite id is required")
    friend = str(raw.get("friend", "")).strip()
    if not friend:
        raise ValueError(f"suite {suite_id}: friend is required")
    source = str(raw.get("source", "local")).strip()
    if source not in {"local", "git"}:
        raise ValueError(f"suite {suite_id}: unsupported source {source!r}")

    semantic_mode = str(raw.get("semantic_mode", "requires_adapter")).strip()
    if semantic_mode not in SUPPORTED_SEMANTIC_MODES:
        raise ValueError(
            f"suite {suite_id}: semantic_mode must be one of "
            f"{sorted(SUPPORTED_SEMANTIC_MODES)}"
        )

    timeout_sec = int(raw.get("timeout_sec", defaults.get("timeout_sec", 900)))
    repeat = int(raw.get("repeat", defaults.get("repeat", 3)))
    if timeout_sec <= 0:
        raise ValueError(f"suite {suite_id}: timeout_sec must be positive")
    if repeat <= 0:
        raise ValueError(f"suite {suite_id}: repeat must be positive")

    runners = _parse_runners(suite_id, raw.get("runners", {}))
    if not runners:
        raise ValueError(f"suite {suite_id}: at least one runner is required")

    prepare_cmds = _parse_command_list(raw.get("prepare_cmds", []), "prepare_cmds")
    suite_env = _parse_env(raw.get("env", defaults.get("env", {})))

    return SuiteSpec(
        id=suite_id,
        friend=friend,
        display_name=str(raw.get("display_name", suite_id)),
        enabled=bool(raw.get("enabled", False)),
        source=source,
        repo_url=_optional_str(raw.get("repo_url")),
        repo_ref=_optional_str(raw.get("repo_ref")),
        local_path=_optional_str(raw.get("local_path")),
        workdir=_optional_str(raw.get("workdir")),
        semantic_mode=semantic_mode,
        adapter_notes=_optional_str(raw.get("adapter_notes")),
        tags=[str(v) for v in raw.get("tags", [])],
        timeout_sec=timeout_sec,
        repeat=repeat,
        env=suite_env,
        prepare_cmds=prepare_cmds,
        runners=runners,
    )


def _optional_str(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def _parse_env(raw_env: Any) -> dict[str, str]:
    if not raw_env:
        return {}
    if not isinstance(raw_env, dict):
        raise ValueError("env must be a table/object of string values")
    parsed: dict[str, str] = {}
    for key, value in raw_env.items():
        parsed[str(key)] = str(value)
    return parsed


def _parse_command_list(raw: Any, field_name: str) -> list[list[str]]:
    if raw in (None, []):
        return []
    if not isinstance(raw, list):
        raise ValueError(f"{field_name} must be an array")
    parsed: list[list[str]] = []
    for idx, entry in enumerate(raw):
        if not isinstance(entry, list) or not entry:
            raise ValueError(f"{field_name}[{idx}] must be a non-empty command array")
        parsed.append([str(part) for part in entry])
    return parsed


def _parse_runners(suite_id: str, raw_runners: Any) -> dict[str, RunnerSpec]:
    if not isinstance(raw_runners, dict):
        raise ValueError(f"suite {suite_id}: runners must be a table/object")
    runners: dict[str, RunnerSpec] = {}
    for runner_name, runner_raw in raw_runners.items():
        runner_name = str(runner_name)
        if not RUNNER_NAME_RE.fullmatch(runner_name):
            raise ValueError(
                f"suite {suite_id}: invalid runner name {runner_name!r}; "
                "use letters, digits, '_', '-', or '.'"
            )
        if not isinstance(runner_raw, dict):
            raise ValueError(
                f"suite {suite_id} runner {runner_name}: runner must be a table/object"
            )
        build_cmd = runner_raw.get("build_cmd")
        run_cmd = runner_raw.get("run_cmd")
        cmd = runner_raw.get("cmd")
        if run_cmd is None and cmd is not None:
            run_cmd = cmd
        json_stdout = bool(runner_raw.get("json_stdout", False))
        structured_stdout = _optional_str(runner_raw.get("structured_stdout"))
        if structured_stdout is not None:
            if structured_stdout != "json":
                raise ValueError(
                    f"suite {suite_id} runner {runner_name}: "
                    "structured_stdout must be 'json'"
                )
            json_stdout = True
        role = str(runner_raw.get("role", "workload")).strip()
        if role not in SUPPORTED_RUNNER_ROLES:
            raise ValueError(
                f"suite {suite_id} runner {runner_name}: role must be one of "
                f"{sorted(SUPPORTED_RUNNER_ROLES)}"
            )
        parsed_build = _parse_single_command(build_cmd, "build_cmd")
        parsed_run = _parse_single_command(run_cmd, "run_cmd")
        runners[runner_name] = RunnerSpec(
            name=runner_name,
            role=role,
            build_cmd=parsed_build,
            run_cmd=parsed_run,
            env=_parse_env(runner_raw.get("env", {})),
            skip_reason=_optional_str(runner_raw.get("skip_reason")),
            json_stdout=json_stdout,
        )
    return runners


def _parse_single_command(raw: Any, field_name: str) -> list[str] | None:
    if raw in (None, []):
        return None
    if not isinstance(raw, list) or not raw:
        raise ValueError(f"{field_name} must be a non-empty command array")
    return [str(part) for part in raw]


def _resolve_tokenized(parts: list[str], tokens: dict[str, str]) -> list[str]:
    return [part.format_map(tokens) for part in parts]


def _resolve_env(raw_env: dict[str, str], tokens: dict[str, str]) -> dict[str, str]:
    return {key: value.format_map(tokens) for key, value in raw_env.items()}

def _select_suites(
    suites: list[SuiteSpec], *, suite_filters: set[str], include_disabled: bool
) -> list[SuiteSpec]:
    if suite_filters:
        selected = [suite for suite in suites if suite.id in suite_filters]
    else:
        selected = suites
    if include_disabled:
        return selected
    return [suite for suite in selected if suite.enabled]


def _parse_keyed_path_overrides(values: list[str], option_name: str) -> dict[str, Path]:
    overrides: dict[str, Path] = {}
    for raw in values:
        if "=" not in raw:
            raise ValueError(
                f"{option_name} entries must be <suite-id>=<path>: {raw!r}"
            )
        suite_id, value = raw.split("=", 1)
        suite_id = suite_id.strip()
        value = value.strip()
        if not suite_id or not value:
            raise ValueError(
                f"{option_name} entries must be <suite-id>=<path>: {raw!r}"
            )
        if suite_id in overrides:
            raise ValueError(f"{option_name} specified multiple times for {suite_id!r}")
        overrides[suite_id] = Path(value).expanduser()
    return overrides


def _parse_keyed_str_overrides(values: list[str], option_name: str) -> dict[str, str]:
    overrides: dict[str, str] = {}
    for raw in values:
        if "=" not in raw:
            raise ValueError(
                f"{option_name} entries must be <suite-id>=<value>: {raw!r}"
            )
        suite_id, value = raw.split("=", 1)
        suite_id = suite_id.strip()
        value = value.strip()
        if not suite_id or not value:
            raise ValueError(
                f"{option_name} entries must be <suite-id>=<value>: {raw!r}"
            )
        if suite_id in overrides:
            raise ValueError(f"{option_name} specified multiple times for {suite_id!r}")
        overrides[suite_id] = value
    return overrides


def _validate_override_targets(
    *,
    suites_by_id: dict[str, SuiteSpec],
    suite_root_overrides: dict[str, Path],
    repo_ref_overrides: dict[str, str],
) -> None:
    for option_name, values in (
        ("--suite-root", suite_root_overrides),
        ("--repo-ref", repo_ref_overrides),
    ):
        unknown = sorted(set(values) - set(suites_by_id))
        if unknown:
            raise ValueError(
                f"{option_name} references unknown suite id(s): {', '.join(unknown)}"
            )
    for suite_id in repo_ref_overrides:
        if suites_by_id[suite_id].source != "git":
            raise ValueError(f"--repo-ref is only valid for git suites: {suite_id}")


def _apply_runner_filter(suite: SuiteSpec, runner_filters: set[str]) -> SuiteSpec:
    if not runner_filters:
        return suite
    runners = {
        name: runner for name, runner in suite.runners.items() if name in runner_filters
    }
    if not runners:
        raise ValueError(
            f"suite {suite.id}: --runner filter selected no configured runners"
        )
    return replace(suite, runners=runners)
