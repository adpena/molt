import argparse
import ast
import datetime as dt
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tomllib
import urllib.request
import zipfile
from pathlib import Path
from typing import Any, Literal

from molt.compat import CompatibilityError
from molt.frontend import SimpleTIRGenerator
from molt.type_facts import (
    collect_type_facts_from_paths,
    load_type_facts,
    write_type_facts,
)

Target = str
ParseCodec = Literal["msgpack", "cbor", "json"]
TypeHintPolicy = Literal["ignore", "trust", "check"]
FallbackPolicy = Literal["error", "bridge"]
BuildProfile = Literal["dev", "release"]
EmitMode = Literal["bin", "obj", "wasm"]
STUB_MODULES = {"molt_buffer", "molt_cbor", "molt_json", "molt_msgpack"}
STUB_PARENT_MODULES = {"molt"}
JSON_SCHEMA_VERSION = "1.0"
CAPABILITY_PROFILES: dict[str, list[str]] = {
    "core": [],
    "net": [
        "network:connect",
        "network:listen",
        "websocket:connect",
        "websocket:listen",
    ],
    "fs": [
        "fs:read",
        "fs:write",
    ],
}
# TODO(tooling, owner:cli, milestone:TL2): align capability profiles with
# docs/spec (process/time/random/db and future host integrations).
CAPABILITY_TOKEN_RE = re.compile(r"^[a-z0-9][a-z0-9:._-]*$")
_OUTPUT_BASE_SAFE_RE = re.compile(r"[^A-Za-z0-9._-]+")


def _emit_json(payload: dict[str, Any], json_output: bool) -> None:
    if json_output:
        print(json.dumps(payload))


def _json_payload(
    command: str,
    status: str,
    *,
    data: dict[str, Any] | None = None,
    warnings: list[str] | None = None,
    errors: list[str] | None = None,
) -> dict[str, Any]:
    payload = {
        "schema_version": JSON_SCHEMA_VERSION,
        "command": command,
        "status": status,
        "data": data or {},
        "warnings": warnings or [],
        "errors": errors or [],
    }
    return payload


def _fail(
    message: str,
    json_output: bool,
    code: int = 2,
    command: str = "molt",
) -> int:
    if json_output:
        payload = _json_payload(
            command,
            "error",
            data={"returncode": code},
            errors=[message],
        )
        _emit_json(payload, json_output=True)
    else:
        print(message, file=sys.stderr)
    return code


def _collect_env_overrides(file_path: str) -> dict[str, str]:
    overrides: dict[str, str] = {}
    try:
        text = Path(file_path).read_text()
    except OSError:
        return overrides
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("# MOLT_ENV:"):
            continue
        payload = stripped[len("# MOLT_ENV:") :].strip()
        for token in payload.split():
            if "=" not in token:
                continue
            key, value = token.split("=", 1)
            overrides[key] = value
    return overrides


def _resolve_python_exe(python_exe: str | None) -> str:
    if not python_exe:
        return sys.executable
    if python_exe[0].isdigit() and os.sep not in python_exe:
        python_exe = f"python{python_exe}"
    if os.sep in python_exe or Path(python_exe).is_absolute():
        candidate = Path(python_exe)
        if candidate.exists():
            return python_exe
        base_exe = getattr(sys, "_base_executable", "")
        if base_exe and Path(base_exe).exists():
            return base_exe
    return python_exe


def _base_env(root: Path, script_path: Path | None = None) -> dict[str, str]:
    env = os.environ.copy()
    paths = [env.get("PYTHONPATH", "")]
    if script_path is not None:
        paths.append(str(script_path.parent))
    paths.extend([str(root / "src"), str(root)])
    env["PYTHONPATH"] = os.pathsep.join(p for p in paths if p)
    return env


def _run_command(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    json_output: bool = False,
    verbose: bool = False,
    label: str | None = None,
    warnings: list[str] | None = None,
) -> int:
    cmd = [str(part) for part in cmd]
    if verbose and not json_output:
        print(f"Running: {shlex.join(cmd)}")
    capture = json_output
    result = subprocess.run(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture,
        text=True,
    )
    if json_output:
        data: dict[str, Any] = {"returncode": result.returncode}
        if result.stdout:
            data["stdout"] = result.stdout
        if result.stderr:
            data["stderr"] = result.stderr
        payload = _json_payload(
            label or cmd[0],
            "ok" if result.returncode == 0 else "error",
            data=data,
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    return result.returncode


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _manifest_errors(manifest: dict[str, Any]) -> list[str]:
    required = [
        "name",
        "version",
        "abi_version",
        "target",
        "capabilities",
        "deterministic",
        "effects",
    ]
    errors: list[str] = []
    for key in required:
        if key not in manifest:
            errors.append(f"missing {key}")
    name = manifest.get("name")
    version = manifest.get("version")
    abi_version = manifest.get("abi_version")
    target = manifest.get("target")
    capabilities = manifest.get("capabilities")
    deterministic = manifest.get("deterministic")
    effects = manifest.get("effects")
    exports = manifest.get("exports")
    if name is not None and not isinstance(name, str):
        errors.append("name must be a string")
    if version is not None and not isinstance(version, str):
        errors.append("version must be a string")
    if abi_version is not None and not isinstance(abi_version, str):
        errors.append("abi_version must be a string")
    if target is not None and not isinstance(target, str):
        errors.append("target must be a string")
    if capabilities is not None:
        if not isinstance(capabilities, list) or not all(
            isinstance(item, str) for item in capabilities
        ):
            errors.append("capabilities must be a list of strings")
    if deterministic is not None and not isinstance(deterministic, bool):
        errors.append("deterministic must be a boolean")
    if effects is not None and not isinstance(effects, (list, str)):
        errors.append("effects must be a list or string")
    if exports is not None:
        if not isinstance(exports, list) or not all(
            isinstance(item, str) for item in exports
        ):
            errors.append("exports must be a list of strings")
    return errors


def _load_manifest(path: Path) -> dict[str, Any] | None:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def _write_zip_member(zf: zipfile.ZipFile, name: str, data: bytes) -> None:
    info = zipfile.ZipInfo(name)
    info.date_time = (1980, 1, 1, 0, 0, 0)
    info.compress_type = zipfile.ZIP_DEFLATED
    zf.writestr(info, data)


def _module_name_from_path(path: Path, roots: list[Path], stdlib_root: Path) -> str:
    resolved = path.resolve()
    rel = None
    try:
        rel = resolved.relative_to(stdlib_root.resolve())
    except ValueError:
        rel = None
    if rel is not None:
        if rel.name == "__init__.py":
            rel = rel.parent
        else:
            rel = rel.with_suffix("")
        if rel.parts:
            return ".".join(rel.parts)
        rel = None
    if rel is None:
        for root in roots:
            try:
                rel = resolved.relative_to(root.resolve())
                break
            except ValueError:
                continue
    if rel is None:
        rel = resolved.with_suffix("")
    if rel.name == "__init__.py":
        rel = rel.parent
    else:
        rel = rel.with_suffix("")
    return ".".join(rel.parts)


def _expand_module_chain(name: str) -> list[str]:
    parts = name.split(".")
    return [".".join(parts[:idx]) for idx in range(1, len(parts) + 1)]


def _find_project_root(start: Path) -> Path:
    for parent in [start] + list(start.parents):
        if (parent / "pyproject.toml").exists() or (parent / ".git").exists():
            return parent
    return start.parent


def _resolve_module_path(module_name: str, roots: list[Path]) -> Path | None:
    parts = module_name.split(".")
    rel = Path(*parts)
    for root in roots:
        mod_path = root / f"{rel}.py"
        if mod_path.exists():
            return mod_path
        pkg_path = root / rel / "__init__.py"
        if pkg_path.exists():
            return pkg_path
    return None


def _collect_imports(tree: ast.AST) -> list[str]:
    imports: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports.append(alias.name)
        elif isinstance(node, ast.ImportFrom):
            if node.level:
                continue
            if node.module:
                imports.append(node.module)
                for alias in node.names:
                    if alias.name != "*":
                        imports.append(f"{node.module}.{alias.name}")
    return imports


def _module_dependencies(
    tree: ast.AST, module_name: str, module_graph: dict[str, Path]
) -> set[str]:
    deps: set[str] = set()
    for name in _collect_imports(tree):
        for candidate in _expand_module_chain(name):
            if candidate == "molt" and module_name.startswith("molt."):
                continue
            if candidate in module_graph and candidate != module_name:
                deps.add(candidate)
            if candidate.startswith("molt.stdlib."):
                stdlib_candidate = candidate[len("molt.stdlib.") :]
                if stdlib_candidate in module_graph and stdlib_candidate != module_name:
                    deps.add(stdlib_candidate)
    return deps


def _collect_func_defaults(tree: ast.AST) -> dict[str, dict[str, Any]]:
    defaults: dict[str, dict[str, Any]] = {}
    if not isinstance(tree, ast.Module):
        return defaults
    for stmt in tree.body:
        if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if stmt.args.vararg or stmt.args.kwarg or stmt.args.kwonlyargs:
            continue
        params = [arg.arg for arg in stmt.args.args]
        default_specs: list[dict[str, Any]] = []
        for expr in stmt.args.defaults:
            if isinstance(expr, ast.Constant):
                default_specs.append({"const": True, "value": expr.value})
            else:
                default_specs.append({"const": False})
        defaults[stmt.name] = {"params": len(params), "defaults": default_specs}
    return defaults


def _topo_sort_modules(
    module_graph: dict[str, Path], module_deps: dict[str, set[str]]
) -> list[str]:
    in_degree = {name: 0 for name in module_graph}
    dependents: dict[str, set[str]] = {name: set() for name in module_graph}
    for name, deps in module_deps.items():
        for dep in deps:
            dependents[dep].add(name)
            in_degree[name] += 1
    ready = sorted(name for name, degree in in_degree.items() if degree == 0)
    order: list[str] = []
    while ready:
        name = ready.pop(0)
        order.append(name)
        for child in sorted(dependents[name]):
            in_degree[child] -= 1
            if in_degree[child] == 0:
                ready.append(child)
    if len(order) != len(module_graph):
        remaining = sorted(name for name in module_graph if name not in order)
        order.extend(remaining)
    return order


def _stdlib_allowlist() -> set[str]:
    allowlist: set[str] = set()
    spec_path = Path("docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md")
    if not spec_path.exists():
        return allowlist
    in_table = False
    for line in spec_path.read_text().splitlines():
        if line.startswith("| Module |"):
            in_table = True
            continue
        if in_table and not line.startswith("|"):
            break
        if not in_table or line.startswith("| ---"):
            continue
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if not parts:
            continue
        module_name = parts[0]
        if not module_name or module_name == "Module":
            continue
        for entry in module_name.split("/"):
            entry = entry.strip()
            if entry:
                allowlist.add(entry)
    return allowlist


def _discover_module_graph(
    entry_path: Path,
    roots: list[Path],
    module_roots: list[Path],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
) -> tuple[dict[str, Path], set[str]]:
    stdlib_root = Path("src/molt/stdlib")
    graph: dict[str, Path] = {}
    skip_modules = skip_modules or set()
    stub_parents = stub_parents or set()
    explicit_imports: set[str] = set()
    queue = [entry_path]
    while queue:
        path = queue.pop()
        module_name = _module_name_from_path(path, module_roots, stdlib_root)
        if module_name in graph:
            continue
        graph[module_name] = path
        try:
            source = path.read_text()
        except OSError:
            continue
        try:
            tree = ast.parse(source)
        except SyntaxError:
            continue
        for name in _collect_imports(tree):
            explicit_imports.add(name)
            for candidate in _expand_module_chain(name):
                if candidate in stub_parents:
                    continue
                if candidate.split(".", 1)[0] in skip_modules:
                    continue
                resolved = None
                if candidate.startswith("molt.stdlib."):
                    stdlib_candidate = candidate[len("molt.stdlib.") :]
                    resolved = _resolve_module_path(stdlib_candidate, [stdlib_root])
                if resolved is None:
                    resolved = _resolve_module_path(candidate, roots)
                if resolved is not None:
                    queue.append(resolved)
    return graph, explicit_imports


def _latest_mtime(paths: list[Path]) -> float:
    latest = 0.0
    for path in paths:
        if path.is_dir():
            for item in path.rglob("*"):
                if item.is_file():
                    latest = max(latest, item.stat().st_mtime)
        elif path.exists():
            latest = max(latest, path.stat().st_mtime)
    return latest


def _check_lockfiles(
    project_root: Path,
    json_output: bool,
    warnings: list[str],
    deterministic: bool,
    command: str,
) -> int | None:
    pyproject = project_root / "pyproject.toml"
    if not pyproject.exists():
        return None
    lock_path = project_root / "uv.lock"
    cargo_lock = project_root / "Cargo.lock"
    missing = []
    if not lock_path.exists():
        missing.append("uv.lock")
    if not cargo_lock.exists():
        missing.append("Cargo.lock")
    if missing and deterministic:
        missing_text = ", ".join(missing)
        return _fail(
            f"Missing lockfiles ({missing_text}); run `uv lock` and ensure Cargo.lock.",
            json_output,
            command=command,
        )
    if missing:
        warnings.append(f"Missing lockfiles: {', '.join(missing)}")
        return None
    # TODO(tooling, owner:cli, milestone:TL2): validate lockfile hashes and enforce
    # uv sync --frozen semantics for deterministic builds.
    try:
        if lock_path.stat().st_mtime < pyproject.stat().st_mtime:
            warnings.append(
                "uv.lock is older than pyproject.toml; run `uv lock` to refresh."
            )
    except OSError:
        warnings.append("Failed to stat uv.lock or pyproject.toml for freshness check.")
    return None


def _load_molt_config(project_root: Path) -> dict[str, Any]:
    config: dict[str, Any] = {}
    molt_toml = project_root / "molt.toml"
    if molt_toml.exists():
        try:
            config.update(tomllib.loads(molt_toml.read_text()))
        except (OSError, tomllib.TOMLDecodeError):
            pass
    pyproject = project_root / "pyproject.toml"
    if pyproject.exists():
        try:
            data = tomllib.loads(pyproject.read_text())
        except (OSError, tomllib.TOMLDecodeError):
            data = {}
        tool_cfg = data.get("tool", {}).get("molt", {})
        if tool_cfg:
            config.setdefault("tool", {})
            config["tool"].setdefault("molt", {})
            config["tool"]["molt"].update(tool_cfg)
    return config


def _config_value(config: dict[str, Any], path: list[str]) -> Any | None:
    current: Any = config
    for key in path:
        if not isinstance(current, dict) or key not in current:
            return None
        current = current[key]
    return current


def _resolve_command_config(config: dict[str, Any], command: str) -> dict[str, Any]:
    cmd_cfg: dict[str, Any] = {}
    direct = _config_value(config, [command])
    if isinstance(direct, dict):
        cmd_cfg.update(direct)
    tool_cfg = _config_value(config, ["tool", "molt", command])
    if isinstance(tool_cfg, dict):
        cmd_cfg.update(tool_cfg)
    return cmd_cfg


def _resolve_build_config(config: dict[str, Any]) -> dict[str, Any]:
    return _resolve_command_config(config, "build")


def _resolve_capabilities_config(config: dict[str, Any]) -> list[str] | None:
    for path in (["capabilities"], ["tool", "molt", "capabilities"]):
        caps = _config_value(config, path)
        if isinstance(caps, list) and all(isinstance(item, str) for item in caps):
            return caps
    return None


def _coerce_bool(value: Any, default: bool) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "on"}
    return default


def _load_capabilities(path: Path) -> tuple[list[str], str] | None:
    try:
        if path.suffix == ".json":
            data = json.loads(path.read_text())
        else:
            data = tomllib.loads(path.read_text())
    except (OSError, json.JSONDecodeError, tomllib.TOMLDecodeError):
        return None
    # TODO(tooling, owner:cli, milestone:TL2): support richer capability manifests
    # (deny lists, per-package grants, and effect annotations).
    caps = None
    if isinstance(data, dict):
        caps = data.get("capabilities")
        if caps is None:
            caps = _config_value(data, ["molt", "capabilities"])
        if caps is None:
            caps = _config_value(data, ["tool", "molt", "capabilities"])
    if isinstance(caps, list) and all(isinstance(item, str) for item in caps):
        return caps, str(path)
    return None


def _expand_capabilities(items: list[str]) -> tuple[list[str], list[str]]:
    expanded: list[str] = []
    profiles: list[str] = []
    for item in items:
        key = item.strip()
        if not key:
            continue
        profile = CAPABILITY_PROFILES.get(key)
        if profile is not None:
            profiles.append(key)
            expanded.extend(profile)
        else:
            expanded.append(key)
    # Preserve order while de-duplicating.
    seen: set[str] = set()
    deduped: list[str] = []
    for cap in expanded:
        if cap in seen:
            continue
        seen.add(cap)
        deduped.append(cap)
    return deduped, profiles


def _parse_capabilities(
    capabilities: str | list[str] | None,
) -> tuple[list[str] | None, list[str], str | None, list[str]]:
    if not capabilities:
        return None, [], None, []
    source: str | None
    items: list[str]
    if isinstance(capabilities, list):
        items = [item.strip() for item in capabilities if item.strip()]
        source = "config"
    else:
        path = Path(capabilities)
        if path.exists():
            loaded = _load_capabilities(path)
            if loaded is None:
                return None, [], str(path), ["failed to load capabilities file"]
            items, source = loaded
        else:
            tokens = re.split(r"[,\s]+", capabilities)
            items = [token for token in tokens if token]
            source = "inline"
    expanded, profiles = _expand_capabilities(items)
    errors: list[str] = []
    for cap in expanded:
        if not CAPABILITY_TOKEN_RE.match(cap):
            errors.append(f"invalid capability token: {cap}")
    return expanded, profiles, source, errors


def _ensure_runtime_lib(
    runtime_lib: Path,
    target_triple: str | None,
    json_output: bool,
    profile: BuildProfile,
) -> bool:
    sources = [
        Path("runtime/molt-runtime/src"),
        Path("runtime/molt-runtime/Cargo.toml"),
        Path("runtime/molt-obj-model/src"),
        Path("runtime/molt-obj-model/Cargo.toml"),
    ]
    latest_src = _latest_mtime(sources)
    lib_mtime = runtime_lib.stat().st_mtime if runtime_lib.exists() else 0.0
    if lib_mtime >= latest_src:
        return True
    cmd = ["cargo", "build", "-p", "molt-runtime"]
    if profile == "release":
        cmd.append("--release")
    if target_triple:
        cmd.extend(["--target", target_triple])
    build = subprocess.run(cmd)
    if build.returncode != 0:
        if not json_output:
            print("Runtime build failed", file=sys.stderr)
        return False
    return True


def _append_rustflags(env: dict[str, str], flags: str) -> None:
    existing = env.get("RUSTFLAGS", "")
    joined = f"{existing} {flags}".strip()
    env["RUSTFLAGS"] = joined


def _ensure_runtime_wasm(
    runtime_wasm: Path,
    *,
    reloc: bool,
    json_output: bool,
    profile: BuildProfile,
) -> bool:
    root = Path(__file__).resolve().parents[2]
    sources = [
        root / "runtime/molt-runtime/src",
        root / "runtime/molt-runtime/Cargo.toml",
        root / "runtime/molt-obj-model/src",
        root / "runtime/molt-obj-model/Cargo.toml",
    ]
    latest_src = _latest_mtime(sources)
    wasm_mtime = runtime_wasm.stat().st_mtime if runtime_wasm.exists() else 0.0
    if wasm_mtime >= latest_src:
        return True
    env = os.environ.copy()
    flags = "-C link-arg=--import-memory -C link-arg=--import-table"
    if reloc:
        flags = (
            f"{flags} -C link-arg=--relocatable -C link-arg=--no-gc-sections"
            " -C relocation-model=pic"
        )
    else:
        flags = f"{flags} -C link-arg=--growable-table"
    _append_rustflags(env, flags)
    cmd = [
        "cargo",
        "build",
        "--package",
        "molt-runtime",
        "--target",
        "wasm32-wasip1",
    ]
    if profile == "release":
        cmd.append("--release")
    build = subprocess.run(cmd, cwd=root, env=env, capture_output=True, text=True)
    if build.returncode != 0:
        if not json_output:
            err = build.stderr.strip() or build.stdout.strip()
            if err:
                print(err, file=sys.stderr)
            print("Runtime wasm build failed", file=sys.stderr)
        return False
    profile_dir = _cargo_profile_dir(profile)
    src = root / "target" / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"
    if not src.exists():
        if not json_output:
            print(
                "Runtime wasm build succeeded but artifact is missing.", file=sys.stderr
            )
        return False
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, runtime_wasm)
    return True


def _read_wasm_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varuint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, offset
        shift += 7
        if shift > 35:
            raise ValueError("varuint too large")


def _read_wasm_string(data: bytes, offset: int) -> tuple[str, int]:
    length, offset = _read_wasm_varuint(data, offset)
    end = offset + length
    if end > len(data):
        raise ValueError("Unexpected EOF while reading string")
    return data[offset:end].decode("utf-8"), end


def _read_wasm_table_min(path: Path) -> int | None:
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if len(data) < 8 or data[:4] != b"\0asm" or data[4:8] != b"\x01\x00\x00\x00":
        return None
    offset = 8
    try:
        while offset < len(data):
            section_id = data[offset]
            offset += 1
            size, offset = _read_wasm_varuint(data, offset)
            end = offset + size
            if end > len(data):
                raise ValueError("Unexpected EOF while reading section")
            if section_id != 2:
                offset = end
                continue
            payload = data[offset:end]
            offset = end
            cursor = 0
            count, cursor = _read_wasm_varuint(payload, cursor)
            for _ in range(count):
                module, cursor = _read_wasm_string(payload, cursor)
                name, cursor = _read_wasm_string(payload, cursor)
                if cursor >= len(payload):
                    raise ValueError("Unexpected EOF while reading import")
                kind = payload[cursor]
                cursor += 1
                if kind == 0:
                    _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 1:
                    if cursor >= len(payload):
                        raise ValueError("Unexpected EOF while reading table type")
                    cursor += 1
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    minimum, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                    if module == "env" and name == "__indirect_function_table":
                        return minimum
                elif kind == 2:
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    _, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 3:
                    if cursor + 2 > len(payload):
                        raise ValueError("Unexpected EOF while reading global type")
                    cursor += 2
                else:
                    raise ValueError("Unknown import kind")
    except ValueError:
        return None
    return None


def _cargo_profile_dir(profile: BuildProfile) -> str:
    return "release" if profile == "release" else "debug"


def _resolve_env_path(var: str, default: Path) -> Path:
    value = os.environ.get(var)
    if not value:
        return default
    path = Path(value).expanduser()
    if not path.is_absolute():
        path = (Path.cwd() / path).absolute()
    return path


def _safe_output_base(name: str) -> str:
    cleaned = _OUTPUT_BASE_SAFE_RE.sub("_", name)
    return cleaned or "molt"


def _default_molt_home() -> Path:
    return _resolve_env_path("MOLT_HOME", Path.home() / ".molt")


def _default_molt_bin() -> Path:
    return _resolve_env_path("MOLT_BIN", _default_molt_home() / "bin")


def _default_molt_cache() -> Path:
    cache_override = os.environ.get("MOLT_CACHE")
    if cache_override:
        return _resolve_env_path("MOLT_CACHE", Path())
    if sys.platform == "darwin":
        base = Path.home() / "Library" / "Caches"
    else:
        xdg = os.environ.get("XDG_CACHE_HOME")
        if xdg:
            base = Path(xdg).expanduser()
            if not base.is_absolute():
                base = (Path.cwd() / base).absolute()
        else:
            base = Path.home() / ".cache"
    return base / "molt"


def _default_build_root(output_base: str) -> Path:
    safe_base = _safe_output_base(output_base)
    return _default_molt_home() / "build" / safe_base


def _resolve_cache_root(project_root: Path, cache_dir: str | None) -> Path:
    if not cache_dir:
        return _default_molt_cache()
    path = Path(cache_dir).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    return path


def _resolve_out_dir(project_root: Path, out_dir: str | Path | None) -> Path | None:
    if not out_dir:
        return None
    path = Path(out_dir).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    path.mkdir(parents=True, exist_ok=True)
    return path


def _resolve_output_roots(
    project_root: Path, out_dir: Path | None, output_base: str
) -> tuple[Path, Path]:
    if out_dir is not None:
        return out_dir, out_dir
    artifacts_root = _default_build_root(output_base)
    bin_root = _default_molt_bin()
    artifacts_root.mkdir(parents=True, exist_ok=True)
    bin_root.mkdir(parents=True, exist_ok=True)
    return artifacts_root, bin_root


def _resolve_output_path(
    output: str | None,
    default: Path,
    *,
    out_dir: Path | None,
    project_root: Path,
) -> Path:
    if not output:
        return default
    path = Path(output).expanduser()
    if not path.is_absolute():
        base = out_dir if out_dir is not None else project_root
        path = base / path
    return path


def _cache_fingerprint() -> str:
    root = Path(__file__).resolve().parents[2]
    paths = [
        root / "runtime" / "molt-backend" / "src" / "lib.rs",
        root / "runtime" / "molt-backend" / "src" / "wasm.rs",
        root / "runtime" / "molt-backend" / "Cargo.toml",
        root / "runtime" / "molt-runtime" / "src" / "lib.rs",
        root / "runtime" / "molt-runtime" / "Cargo.toml",
    ]
    parts: list[str] = []
    for path in paths:
        try:
            stat = path.stat()
        except OSError:
            continue
        try:
            rel = path.relative_to(root).as_posix()
        except ValueError:
            rel = path.as_posix()
        parts.append(f"{rel}:{stat.st_mtime_ns}:{stat.st_size}")
    return "|".join(parts)


def _cache_key(
    ir: dict[str, Any],
    target: str,
    target_triple: str | None,
    variant: str = "",
) -> str:
    payload = json.dumps(ir, sort_keys=True, separators=(",", ":")).encode("utf-8")
    suffix = target_triple or target
    if variant:
        suffix = f"{suffix}:{variant}"
    fingerprint = _cache_fingerprint().encode("utf-8")
    digest = hashlib.sha256(
        payload + b"|" + suffix.encode("utf-8") + b"|" + fingerprint
    ).hexdigest()
    return digest


def _ensure_rustup_target(target_triple: str, warnings: list[str]) -> bool:
    rustup_path = shutil.which("rustup")
    if not rustup_path:
        warnings.append(f"rustup not found; cannot ensure target {target_triple}")
        return False
    try:
        result = subprocess.run(
            ["rustup", "target", "list", "--installed"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        warnings.append(f"Failed to query rustup targets: {exc}")
        return False
    installed = result.stdout.split()
    if target_triple in installed:
        return True
    try:
        add = subprocess.run(
            ["rustup", "target", "add", target_triple],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        warnings.append(f"Failed to add rustup target {target_triple}: {exc}")
        return False
    if add.returncode != 0:
        detail = (add.stderr or add.stdout).strip() or "unknown error"
        warnings.append(f"rustup target add failed for {target_triple}: {detail}")
        return False
    return True


def _strip_arch_flags(args: list[str]) -> list[str]:
    cleaned: list[str] = []
    skip_next = False
    for arg in args:
        if skip_next:
            skip_next = False
            continue
        if arg == "-arch":
            skip_next = True
            continue
        if arg.startswith("-arch="):
            continue
        cleaned.append(arg)
    return cleaned


def _zig_target_query(target_triple: str) -> str:
    # TODO(tooling, owner:cli, milestone:TL2): replace with a comprehensive
    # target-triple to zig target query mapping (vendor/abi/sysroot aware).
    parts = target_triple.split("-")
    if len(parts) < 3:
        return target_triple
    arch = parts[0]
    os_name = parts[2]
    env = parts[3] if len(parts) > 3 else None
    if os_name in {"darwin", "macosx"}:
        return f"{arch}-macos"
    if env:
        return f"{arch}-{os_name}-{env}"
    return f"{arch}-{os_name}"


def _detect_macos_arch(obj_path: Path) -> str | None:
    try:
        result = subprocess.run(
            ["lipo", "-archs", str(obj_path)],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    archs = result.stdout.strip().split()
    return archs[0] if archs else None


def _detect_macos_deployment_target() -> str | None:
    env_target = os.environ.get("MOLT_MACOSX_DEPLOYMENT_TARGET")
    if env_target:
        return env_target
    env_target = os.environ.get("MACOSX_DEPLOYMENT_TARGET")
    if env_target:
        return env_target
    try:
        result = subprocess.run(
            ["xcrun", "--show-sdk-version"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    version = result.stdout.strip()
    return version or None


def build(
    file_path: str,
    target: Target = "native",
    parse_codec: ParseCodec = "msgpack",
    type_hint_policy: TypeHintPolicy = "ignore",
    fallback_policy: FallbackPolicy = "error",
    type_facts_path: str | None = None,
    output: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    trusted: bool = False,
    capabilities: str | list[str] | None = None,
    cache: bool = True,
    cache_dir: str | None = None,
    cache_report: bool = False,
    emit_ir: str | None = None,
    emit: EmitMode | None = None,
    out_dir: str | None = None,
    profile: BuildProfile = "release",
    linked: bool = False,
    linked_output: str | None = None,
    require_linked: bool = False,
) -> int:
    if profile not in {"dev", "release"}:
        return _fail(f"Invalid build profile: {profile}", json_output, command="build")
    source_path = Path(file_path)
    if not source_path.exists():
        return _fail(f"File not found: {source_path}", json_output, command="build")

    stdlib_root = Path("src/molt/stdlib")
    warnings: list[str] = []
    try:
        entry_source = source_path.read_text()
    except OSError as exc:
        return _fail(
            f"Failed to read entry module {source_path}: {exc}",
            json_output,
            command="build",
        )
    try:
        entry_tree = ast.parse(entry_source)
    except SyntaxError as exc:
        return _fail(
            f"Syntax error in {source_path}: {exc}",
            json_output,
            command="build",
        )
    entry_imports = set(_collect_imports(entry_tree))
    stub_parents = STUB_PARENT_MODULES - entry_imports
    project_root = _find_project_root(source_path.resolve())
    lock_error = _check_lockfiles(
        project_root, json_output, warnings, deterministic, "build"
    )
    if lock_error is not None:
        return lock_error
    capabilities_list: list[str] | None = None
    capabilities_source = None
    capability_profiles: list[str] = []
    if capabilities:
        parsed, profiles, source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="build",
            )
        capabilities_list = parsed
        capability_profiles = profiles
        capabilities_source = source
    cwd_root = _find_project_root(Path.cwd())
    module_roots: list[Path] = []
    for root in (project_root, cwd_root):
        src_root = root / "src"
        if src_root.exists():
            module_roots.append(src_root)
    module_roots.append(source_path.parent)
    module_roots = list(dict.fromkeys(root.resolve() for root in module_roots))
    roots = module_roots + [stdlib_root]
    module_graph, _explicit_imports = _discover_module_graph(
        source_path,
        roots,
        module_roots,
        skip_modules=STUB_MODULES,
        stub_parents=stub_parents,
    )
    if verbose and not json_output:
        print(f"Project root: {project_root}")
        print(f"Module roots: {', '.join(str(root) for root in module_roots)}")
        print(f"Modules discovered: {len(module_graph)}")
    entry_module = _module_name_from_path(source_path, module_roots, stdlib_root)
    output_base = entry_module.rsplit(".", 1)[-1] or source_path.stem
    out_dir_path = _resolve_out_dir(project_root, out_dir)
    artifacts_root, bin_root = _resolve_output_roots(
        project_root, out_dir_path, output_base
    )
    is_wasm = target == "wasm"
    if trusted and is_wasm:
        return _fail(
            "Trusted mode is not supported for wasm targets",
            json_output,
            command="build",
        )
    if require_linked and not is_wasm:
        return _fail(
            "--require-linked is only supported for wasm targets",
            json_output,
            command="build",
        )
    if linked_output and not linked and not require_linked:
        return _fail(
            "--linked-output requires --linked",
            json_output,
            command="build",
        )
    if linked and not is_wasm:
        return _fail(
            "Linked output is only supported for wasm targets",
            json_output,
            command="build",
        )
    if require_linked and not linked:
        linked = True
    target_triple = None if target in {"native", "wasm"} else target
    emit_mode = emit or ("wasm" if is_wasm else "bin")
    if emit_mode not in {"bin", "obj", "wasm"}:
        return _fail(
            f"Invalid emit mode: {emit_mode}",
            json_output,
            command="build",
        )
    if is_wasm and emit_mode != "wasm":
        return _fail(
            f"Invalid emit mode for wasm target: {emit_mode}",
            json_output,
            command="build",
        )
    if not is_wasm and emit_mode == "wasm":
        return _fail(
            "emit=wasm requires --target wasm",
            json_output,
            command="build",
        )
    backend_output = Path("output.wasm" if is_wasm else "output.o")
    output_binary: Path | None = None
    linked_output_path: Path | None = None
    if is_wasm:
        output_wasm = _resolve_output_path(
            output,
            artifacts_root / "output.wasm",
            out_dir=out_dir_path,
            project_root=project_root,
        )
        output_artifact = output_wasm
        if linked:
            stem = output_wasm.stem
            if stem.endswith("_linked"):
                stem = stem[: -len("_linked")]
            linked_output_path = output_wasm.with_name(
                f"{stem}_linked{output_wasm.suffix}"
            )
            if linked_output is not None:
                linked_output_path = _resolve_output_path(
                    linked_output,
                    linked_output_path,
                    out_dir=out_dir_path,
                    project_root=project_root,
                )
    else:
        output_obj = artifacts_root / "output.o"
        if emit_mode == "obj":
            output_obj = _resolve_output_path(
                output,
                output_obj,
                out_dir=out_dir_path,
                project_root=project_root,
            )
        output_artifact = output_obj
        if emit_mode == "bin":
            output_binary = _resolve_output_path(
                output,
                bin_root / f"{output_base}_molt",
                out_dir=out_dir_path,
                project_root=project_root,
            )
    for path in (output_artifact, output_binary):
        if path is not None and path.parent != Path("."):
            path.parent.mkdir(parents=True, exist_ok=True)
    emit_ir_path: Path | None = None
    if emit_ir:
        emit_ir_path = Path(emit_ir)
        if not emit_ir_path.is_absolute():
            emit_ir_path = artifacts_root / emit_ir_path
        if emit_ir_path.parent != Path("."):
            emit_ir_path.parent.mkdir(parents=True, exist_ok=True)
    for stub in stub_parents:
        if stub != entry_module:
            module_graph.pop(stub, None)
    known_modules = set(module_graph.keys())
    stdlib_allowlist = _stdlib_allowlist()
    stdlib_allowlist.update(STUB_MODULES)
    stdlib_allowlist.update(stub_parents)
    stdlib_allowlist.add("molt.stdlib")
    module_deps: dict[str, set[str]] = {}
    known_func_defaults: dict[str, dict[str, dict[str, Any]]] = {}
    for module_name, module_path in module_graph.items():
        try:
            source = module_path.read_text()
        except OSError as exc:
            return _fail(
                f"Failed to read module {module_path}: {exc}",
                json_output,
                command="build",
            )
        try:
            tree = ast.parse(source)
        except SyntaxError as exc:
            return _fail(
                f"Syntax error in {module_path}: {exc}",
                json_output,
                command="build",
            )
        module_deps[module_name] = _module_dependencies(tree, module_name, module_graph)
        known_func_defaults[module_name] = _collect_func_defaults(tree)
    module_order = _topo_sort_modules(module_graph, module_deps)
    type_facts = None
    if type_facts_path is None and type_hint_policy in {"trust", "check"}:
        type_facts, ty_ok = _collect_type_facts_for_build(
            list(module_graph.values()), type_hint_policy, source_path
        )
        if type_facts is None and type_hint_policy == "trust":
            return _fail(
                "Type facts unavailable; refusing trusted build.",
                json_output,
                command="build",
            )
        if type_hint_policy == "trust" and not ty_ok:
            return _fail(
                "ty check failed; refusing trusted build.",
                json_output,
                command="build",
            )
        if type_hint_policy == "check" and not ty_ok:
            warning = "ty check failed; continuing with guarded hints only."
            warnings.append(warning)
            if not json_output:
                print(warning, file=sys.stderr)
    if type_facts_path is not None:
        facts_path = Path(type_facts_path)
        if not facts_path.exists():
            return _fail(
                f"Type facts not found: {facts_path}",
                json_output,
                command="build",
            )
        try:
            type_facts = load_type_facts(facts_path)
        except (OSError, json.JSONDecodeError, ValueError) as exc:
            return _fail(
                f"Failed to load type facts: {exc}",
                json_output,
                command="build",
            )

    functions: list[dict[str, Any]] = []
    # Normalize code-slot IDs across modules to keep tracebacks consistent.
    global_code_ids: dict[str, int] = {}
    global_code_id_counter = 0

    def _register_global_code_id(symbol: str) -> int:
        nonlocal global_code_id_counter
        code_id = global_code_ids.get(symbol)
        if code_id is None:
            code_id = global_code_id_counter
            global_code_ids[symbol] = code_id
            global_code_id_counter += 1
        return code_id

    def _remap_module_code_ops(
        module_name: str,
        funcs: list[dict[str, Any]],
        local_id_to_symbol: dict[int, str],
    ) -> None:
        for func in funcs:
            ops = func.get("ops", [])
            remapped_ops: list[dict[str, Any]] = []
            for op in ops:
                kind = op.get("kind")
                if kind == "code_slots_init":
                    continue
                if kind == "call":
                    symbol = op.get("s_value")
                    if symbol:
                        op["value"] = _register_global_code_id(symbol)
                elif kind == "code_slot_set":
                    local_id = op.get("value")
                    symbol = local_id_to_symbol.get(local_id)
                    if symbol is None:
                        raise ValueError(
                            "Missing code symbol for id "
                            f"{local_id} in module {module_name}"
                        )
                    op["value"] = _register_global_code_id(symbol)
                remapped_ops.append(op)
            func["ops"] = remapped_ops

    enable_phi = not is_wasm
    if target_triple:
        _ensure_rustup_target(target_triple, warnings)
    known_classes: dict[str, Any] = {}
    for module_name in module_order:
        module_path = module_graph[module_name]
        try:
            source = module_path.read_text()
        except OSError as exc:
            return _fail(
                f"Failed to read module {module_path}: {exc}",
                json_output,
                command="build",
            )
        try:
            tree = ast.parse(source)
        except SyntaxError as exc:
            return _fail(
                f"Syntax error in {module_path}: {exc}",
                json_output,
                command="build",
            )
        gen = SimpleTIRGenerator(
            parse_codec=parse_codec,
            type_hint_policy=type_hint_policy,
            fallback_policy=fallback_policy,
            source_path=str(module_path),
            type_facts=type_facts,
            module_name=module_name,
            entry_module=entry_module,
            enable_phi=enable_phi,
            known_modules=known_modules,
            known_classes=known_classes,
            stdlib_allowlist=stdlib_allowlist,
            known_func_defaults=known_func_defaults,
        )
        try:
            gen.visit(tree)
        except CompatibilityError as exc:
            return _fail(str(exc), json_output, command="build")
        ir = gen.to_json()
        init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
        local_code_ids = dict(gen.func_code_ids)
        if "molt_main" in local_code_ids:
            local_code_ids[init_symbol] = local_code_ids.pop("molt_main")
        local_id_to_symbol = {
            code_id: symbol for symbol, code_id in local_code_ids.items()
        }
        try:
            _remap_module_code_ops(module_name, ir["functions"], local_id_to_symbol)
        except ValueError as exc:
            return _fail(str(exc), json_output, command="build")
        for func in ir["functions"]:
            if func["name"] == "molt_main":
                func["name"] = init_symbol
        functions.extend(ir["functions"])
        for class_name in gen.local_class_names:
            known_classes[class_name] = gen.classes[class_name]

    entry_init = SimpleTIRGenerator.module_init_symbol(entry_module)
    entry_ops = [
        {
            "kind": "call",
            "s_value": "molt_runtime_init",
            "args": [],
            "out": "v0",
            "value": _register_global_code_id("molt_runtime_init"),
        },
        {
            "kind": "call",
            "s_value": entry_init,
            "args": [],
            "out": "v1",
            "value": _register_global_code_id(entry_init),
        },
        {
            "kind": "call",
            "s_value": "molt_runtime_shutdown",
            "args": [],
            "out": "v2",
            "value": _register_global_code_id("molt_runtime_shutdown"),
        },
        {"kind": "ret_void"},
    ]
    entry_ops.insert(1, {"kind": "code_slots_init", "value": len(global_code_ids)})
    functions.append({"name": "molt_main", "params": [], "ops": entry_ops})
    ir = {"functions": functions}
    if emit_ir_path is not None:
        try:
            emit_ir_path.write_text(json.dumps(ir, indent=2) + "\n")
        except OSError as exc:
            return _fail(f"Failed to write IR: {exc}", json_output, command="build")
    cache_hit = False
    cache_key = None
    cache_path: Path | None = None
    if cache:
        cache_variant = "linked" if linked else ""
        cache_key = _cache_key(ir, target, target_triple, cache_variant)
        cache_root = _resolve_cache_root(project_root, cache_dir)
        try:
            cache_root.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            warnings.append(f"Cache disabled: {exc}")
            cache = False
        else:
            ext = "wasm" if is_wasm else "o"
            cache_path = cache_root / f"{cache_key}.{ext}"
            if cache_path.exists():
                try:
                    shutil.copy2(cache_path, output_artifact)
                    cache_hit = True
                except OSError as exc:
                    warnings.append(f"Cache copy failed: {exc}")
                    cache_hit = False
    if (verbose or cache_report) and not json_output:
        if not cache:
            print("Cache: disabled")
        elif cache_key:
            cache_state = "hit" if cache_hit else "miss"
            cache_detail = f" ({cache_key})" if cache_key else ""
            print(f"Cache: {cache_state}{cache_detail}")

    # 2. Backend: JSON IR -> output.o / output.wasm
    if not cache_hit:
        backend_env = None
        reloc_requested = is_wasm and (
            linked or os.environ.get("MOLT_WASM_LINK") == "1"
        )
        if reloc_requested:
            backend_env = os.environ.copy()
            backend_env["MOLT_WASM_LINK"] = "1"
            if "MOLT_WASM_TABLE_BASE" not in backend_env:
                root = Path(__file__).resolve().parents[2]
                runtime_reloc = root / "wasm" / "molt_runtime_reloc.wasm"
                if linked and not _ensure_runtime_wasm(
                    runtime_reloc,
                    reloc=True,
                    json_output=json_output,
                    profile=profile,
                ):
                    return _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
                if runtime_reloc.exists():
                    table_base = _read_wasm_table_min(runtime_reloc)
                    if table_base is not None:
                        backend_env["MOLT_WASM_TABLE_BASE"] = str(table_base)
                    else:
                        warnings.append(
                            "Failed to read runtime table size; using default table base."
                        )
        cmd = ["cargo", "run", "--quiet"]
        if profile == "release":
            cmd.append("--release")
        cmd.extend(["--package", "molt-backend", "--"])
        if is_wasm:
            cmd.extend(["--target", "wasm"])
        elif target_triple:
            cmd.extend(["--target-triple", target_triple])

        backend_process = subprocess.run(
            cmd,
            input=json.dumps(ir),
            text=True,
            capture_output=True,
            env=backend_env,
        )
        if backend_process.returncode != 0:
            if not json_output:
                if backend_process.stderr:
                    print(backend_process.stderr, end="", file=sys.stderr)
                if backend_process.stdout:
                    print(backend_process.stdout, end="")
            return _fail(
                "Backend compilation failed",
                json_output,
                backend_process.returncode or 1,
                command="build",
            )
        if verbose and not json_output:
            if backend_process.stdout:
                print(backend_process.stdout, end="")
            if backend_process.stderr:
                print(backend_process.stderr, end="", file=sys.stderr)
        if not backend_output.exists():
            return _fail("Backend output missing", json_output, command="build")
        if backend_output != output_artifact:
            try:
                if output_artifact.parent != Path("."):
                    output_artifact.parent.mkdir(parents=True, exist_ok=True)
                backend_output.replace(output_artifact)
            except OSError as exc:
                return _fail(
                    f"Failed to move backend output: {exc}",
                    json_output,
                    command="build",
                )
        if cache and cache_path is not None:
            try:
                shutil.copy2(output_artifact, cache_path)
            except OSError as exc:
                warnings.append(f"Cache write failed: {exc}")

    if is_wasm:
        output_wasm = output_artifact
        if linked:
            root = Path(__file__).resolve().parents[2]
            runtime_reloc = root / "wasm" / "molt_runtime_reloc.wasm"
            if not _ensure_runtime_wasm(
                runtime_reloc,
                reloc=True,
                json_output=json_output,
                profile=profile,
            ):
                return _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if linked_output_path is None:
                linked_output_path = output_wasm.with_name("output_linked.wasm")
            if linked_output_path.parent != Path("."):
                linked_output_path.parent.mkdir(parents=True, exist_ok=True)
            tool = root / "tools" / "wasm_link.py"
            link_process = subprocess.run(
                [
                    sys.executable,
                    str(tool),
                    "--runtime",
                    str(runtime_reloc),
                    "--input",
                    str(output_wasm),
                    "--output",
                    str(linked_output_path),
                ],
                cwd=root,
                capture_output=True,
                text=True,
            )
            if link_process.returncode != 0:
                err = link_process.stderr.strip() or link_process.stdout.strip()
                msg = "Wasm link failed"
                if err:
                    msg = f"{msg}: {err}"
                return _fail(msg, json_output, command="build")
            if require_linked and linked_output_path is not None:
                if output_wasm != linked_output_path and output_wasm.exists():
                    try:
                        output_wasm.unlink()
                    except OSError as exc:
                        return _fail(
                            f"Failed to remove unlinked wasm: {exc}",
                            json_output,
                            command="build",
                        )
        primary_output = output_wasm
        if require_linked and linked_output_path is not None:
            primary_output = linked_output_path
        if json_output:
            cache_info: dict[str, Any] = {"enabled": cache, "hit": cache_hit}
            if cache_key:
                cache_info["key"] = cache_key
            if cache_path is not None:
                cache_info["path"] = str(cache_path)
            data = {
                "target": target,
                "target_triple": target_triple,
                "entry": str(source_path),
                "output": str(primary_output),
                "deterministic": deterministic,
                "trusted": trusted,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "capabilities_source": capabilities_source,
                "cache": cache_info,
                "emit": emit_mode,
                "profile": profile,
                "linked": linked,
                "require_linked": require_linked,
            }
            if linked_output_path is not None:
                data["linked_output"] = str(linked_output_path)
            if emit_ir_path is not None:
                data["emit_ir"] = str(emit_ir_path)
            payload = _json_payload(
                "build",
                "ok",
                data=data,
                warnings=warnings,
            )
            _emit_json(payload, json_output)
        else:
            if require_linked:
                print(f"Successfully built {primary_output}")
            else:
                print(f"Successfully built {output_wasm}")
            if linked_output_path is not None and not require_linked:
                print(f"Successfully linked {linked_output_path}")
        return 0

    output_obj = output_artifact
    if emit_mode == "obj":
        if json_output:
            cache_info = {"enabled": cache, "hit": cache_hit}
            if cache_key:
                cache_info["key"] = cache_key
            if cache_path is not None:
                cache_info["path"] = str(cache_path)
            data = {
                "target": target,
                "target_triple": target_triple,
                "entry": str(source_path),
                "output": str(output_obj),
                "deterministic": deterministic,
                "trusted": trusted,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "capabilities_source": capabilities_source,
                "cache": cache_info,
                "emit": emit_mode,
                "profile": profile,
                "artifacts": {"object": str(output_obj)},
            }
            if emit_ir_path is not None:
                data["emit_ir"] = str(emit_ir_path)
            payload = _json_payload(
                "build",
                "ok",
                data=data,
                warnings=warnings,
            )
            _emit_json(payload, json_output)
        else:
            print(f"Successfully built {output_obj}")
        return 0

    # 3. Linking: output.o + main.c -> binary
    trusted_snippet = ""
    trusted_call = ""
    if trusted:
        trusted_snippet = """
static void molt_set_trusted() {
#ifdef _WIN32
    _putenv_s("MOLT_TRUSTED", "1");
#else
    setenv("MOLT_TRUSTED", "1", 1);
#endif
}
"""
        trusted_call = "    molt_set_trusted();\n"
    main_c_content = """
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#include <wchar.h>
#endif
extern unsigned long long molt_runtime_init();
extern unsigned long long molt_runtime_shutdown();
extern void molt_set_argv(int argc, const char** argv);
#ifdef _WIN32
extern void molt_set_argv_utf16(int argc, const wchar_t** argv);
#endif
extern void molt_main();
extern int molt_json_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_msgpack_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_cbor_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern long molt_get_attr_generic(void* obj, const char* attr, long len);
extern unsigned long long molt_alloc(long size);
extern long molt_block_on(void* task);
extern long molt_async_sleep(void* obj);
extern void molt_spawn(void* task);
extern void* molt_chan_new(unsigned long long capacity);
extern long molt_chan_send(void* chan, long val);
extern long molt_chan_recv(void* chan);
extern void molt_print_obj(unsigned long long val);
extern void molt_profile_dump();
/* MOLT_TRUSTED_SNIPPET */

static void molt_finish() {
    const char* profile = getenv("MOLT_PROFILE");
    if (profile != NULL && profile[0] != '\\0' && strcmp(profile, "0") != 0) {
        molt_profile_dump();
    }
    molt_runtime_shutdown();
}

#ifdef _WIN32
int wmain(int argc, wchar_t** argv) {
    /* MOLT_TRUSTED_CALL */
    molt_runtime_init();
    molt_set_argv_utf16(argc, (const wchar_t**)argv);
    molt_main();
    molt_finish();
    return 0;
}
#else
int main(int argc, char** argv) {
    /* MOLT_TRUSTED_CALL */
    molt_runtime_init();
    molt_set_argv(argc, (const char**)argv);
    molt_main();
    molt_finish();
    return 0;
}
#endif
"""
    main_c_content = main_c_content.replace(
        "/* MOLT_TRUSTED_SNIPPET */", trusted_snippet
    )
    main_c_content = main_c_content.replace("/* MOLT_TRUSTED_CALL */", trusted_call)
    stub_path = artifacts_root / "main_stub.c"
    stub_path.write_text(main_c_content)

    if output_binary is None:
        return _fail("Binary output unavailable", json_output, command="build")
    if output_binary.parent != Path("."):
        output_binary.parent.mkdir(parents=True, exist_ok=True)
    profile_dir = _cargo_profile_dir(profile)
    if target_triple:
        runtime_lib = Path("target") / target_triple / profile_dir / "libmolt_runtime.a"
    else:
        runtime_lib = Path("target") / profile_dir / "libmolt_runtime.a"
    if not _ensure_runtime_lib(runtime_lib, target_triple, json_output, profile):
        return _fail("Runtime build failed", json_output, command="build")

    cc = os.environ.get("CC", "clang")
    link_cmd = shlex.split(cc)
    if target_triple:
        # TODO(tooling, owner:cli, milestone:TL2): support sysroot configuration,
        # target-specific flags, and cached runtime cross-builds.
        cross_cc = os.environ.get("MOLT_CROSS_CC")
        target_arg = target_triple
        if cross_cc:
            link_cmd = shlex.split(cross_cc)
        elif shutil.which("zig"):
            link_cmd = ["zig", "cc"]
            target_arg = _zig_target_query(target_triple)
            if target_arg != target_triple:
                warnings.append(
                    f"Zig target normalized to {target_arg} from {target_triple}."
                )
        else:
            return _fail(
                f"Cross-target build requires zig or MOLT_CROSS_CC (missing for {target_triple}).",
                json_output,
                command="build",
            )
        link_cmd.extend(["-target", target_arg])
    cflags = os.environ.get("CFLAGS", "")
    if cflags:
        link_cmd.extend(shlex.split(cflags))
    if sys.platform == "darwin" and not target_triple:
        link_cmd = _strip_arch_flags(link_cmd)
        arch = (
            os.environ.get("MOLT_ARCH")
            or _detect_macos_arch(output_obj)
            or platform.machine()
        )
        link_cmd.extend(["-arch", arch])
        deployment_target = _detect_macos_deployment_target()
        if deployment_target:
            link_cmd.append(f"-mmacosx-version-min={deployment_target}")
    link_cmd.extend(
        [str(stub_path), str(output_obj), str(runtime_lib), "-o", str(output_binary)]
    )
    if target_triple:
        if "apple" in target_triple or "darwin" in target_triple:
            link_cmd.append("-lc++")
        elif "linux" in target_triple:
            link_cmd.append("-lstdc++")
            link_cmd.append("-lm")
    else:
        if sys.platform == "darwin":
            link_cmd.append("-lc++")
        elif sys.platform.startswith("linux"):
            link_cmd.append("-lstdc++")
            link_cmd.append("-lm")

    link_process = subprocess.run(
        link_cmd,
        capture_output=json_output,
        text=True,
    )

    if link_process.returncode == 0:
        if json_output:
            cache_info = {"enabled": cache, "hit": cache_hit}
            if cache_key:
                cache_info["key"] = cache_key
            if cache_path is not None:
                cache_info["path"] = str(cache_path)
            data: dict[str, Any] = {
                "target": target,
                "target_triple": target_triple,
                "entry": str(source_path),
                "output": str(output_binary),
                "artifacts": {
                    "object": str(output_obj),
                    "stub": str(stub_path),
                    "runtime": str(runtime_lib),
                },
                "deterministic": deterministic,
                "trusted": trusted,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "capabilities_source": capabilities_source,
                "cache": cache_info,
                "emit": emit_mode,
                "profile": profile,
            }
            if emit_ir_path is not None:
                data["emit_ir"] = str(emit_ir_path)
            if link_process.stdout:
                data["stdout"] = link_process.stdout
            if link_process.stderr:
                data["stderr"] = link_process.stderr
            payload = _json_payload(
                "build",
                "ok",
                data=data,
                warnings=warnings,
            )
            _emit_json(payload, json_output)
        else:
            print(f"Successfully built {output_binary}")
    else:
        if json_output:
            data: dict[str, Any] = {
                "target": target,
                "entry": str(source_path),
                "returncode": link_process.returncode,
                "emit": emit_mode,
                "profile": profile,
                "trusted": trusted,
            }
            data["cache"] = {
                "enabled": cache,
                "hit": cache_hit,
                "key": cache_key,
            }
            if cache_path is not None:
                data["cache"]["path"] = str(cache_path)
            if link_process.stdout:
                data["stdout"] = link_process.stdout
            if link_process.stderr:
                data["stderr"] = link_process.stderr
            payload = _json_payload(
                "build",
                "error",
                data=data,
                errors=["Linking failed"],
            )
            _emit_json(payload, json_output)
        else:
            print("Linking failed", file=sys.stderr)

    return link_process.returncode


def run_script(
    file_path: str,
    python_exe: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    no_shims: bool = False,
    compiled: bool = False,
    compiled_args: bool = False,
    trusted: bool = False,
    build_args: list[str] | None = None,
) -> int:
    source_path = Path(file_path)
    if not source_path.exists():
        return _fail(f"File not found: {source_path}", json_output, command="run")
    root = _find_project_root(source_path.resolve())
    env = _base_env(root, source_path)
    env.update(_collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"

    if compiled:
        build_args = list(build_args or [])
        if trusted and not _build_args_has_trusted_flag(build_args):
            build_args.append("--trusted")
        build_cmd = [sys.executable, "-m", "molt.cli", "build", *build_args, file_path]
        build_res = subprocess.run(
            build_cmd,
            env=env,
            cwd=root,
            capture_output=json_output,
            text=json_output,
        )
        if build_res.returncode != 0:
            if json_output:
                data: dict[str, Any] = {"returncode": build_res.returncode}
                if build_res.stdout:
                    data["build_stdout"] = build_res.stdout
                if build_res.stderr:
                    data["build_stderr"] = build_res.stderr
                payload = _json_payload(
                    "run",
                    "error",
                    data=data,
                    errors=["build failed"],
                )
                _emit_json(payload, json_output=True)
            elif build_res.stdout:
                print(build_res.stdout, end="")
                if build_res.stderr:
                    print(build_res.stderr, end="", file=sys.stderr)
            return build_res.returncode
        emit_arg = _extract_emit_arg(build_args)
        if emit_arg and emit_arg != "bin":
            return _fail(
                f"Compiled run requires emit=bin (got {emit_arg})",
                json_output,
                command="run",
            )
        output_binary = _extract_output_arg(build_args)
        out_dir = _extract_out_dir_arg(build_args)
        out_dir_path = _resolve_out_dir(root, out_dir)
        _artifacts_root, bin_root = _resolve_output_roots(
            root, out_dir_path, source_path.stem
        )
        output_binary = _resolve_output_path(
            str(output_binary) if output_binary is not None else None,
            bin_root / f"{source_path.stem}_molt",
            out_dir=out_dir_path,
            project_root=root,
        )
        # TODO(tooling, owner:cli, milestone:TL2): plumb argv support for compiled
        # binaries once the runtime exposes argument handling.
        ignored_warning = None
        if script_args and not compiled_args:
            ignored_warning = "Ignoring script args for compiled binary run."
            if verbose and not json_output:
                print(ignored_warning)
        return _run_command(
            [str(output_binary), *script_args]
            if compiled_args
            else [str(output_binary)],
            env=env,
            cwd=root,
            json_output=json_output,
            verbose=verbose,
            label="run",
            warnings=[ignored_warning] if ignored_warning else None,
        )

    python_exe = _resolve_python_exe(python_exe)
    if no_shims:
        cmd = [python_exe, str(source_path), *script_args]
    else:
        bootstrap = (
            "import runpy, sys; "
            "import molt.shims as shims; "
            "shims.install(); "
            "runpy.run_path(sys.argv[1], run_name='__main__')"
        )
        cmd = [python_exe, "-c", bootstrap, str(source_path), *script_args]
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="run",
    )


def diff(
    file_path: str | None,
    python_version: str | None,
    trusted: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_project_root(Path.cwd())
    env = _base_env(root)
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    cmd = [sys.executable, "tests/molt_diff.py"]
    if python_version:
        cmd.extend(["--python-version", python_version])
    if file_path:
        cmd.append(file_path)
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="diff",
    )


def lint(json_output: bool = False, verbose: bool = False) -> int:
    root = _find_project_root(Path.cwd())
    cmd = [sys.executable, "tools/dev.py", "lint"]
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="lint",
    )


def test(
    suite: str,
    file_path: str | None,
    python_version: str | None,
    pytest_args: list[str],
    trusted: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_project_root(Path.cwd())
    env = _base_env(root)
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if suite == "dev":
        cmd = [sys.executable, "tools/dev.py", "test"]
    elif suite == "diff":
        cmd = [sys.executable, "tests/molt_diff.py"]
        if python_version:
            cmd.extend(["--python-version", python_version])
        if file_path:
            cmd.append(file_path)
    else:
        cmd = ["uv", "run", "--python", "3.12", "pytest", "-q"]
        if file_path:
            cmd.append(file_path)
        cmd.extend(pytest_args)
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="test",
    )


def bench(
    wasm: bool,
    bench_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_project_root(Path.cwd())
    tool = "tools/bench_wasm.py" if wasm else "tools/bench.py"
    cmd = [sys.executable, tool, *bench_args]
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="bench",
    )


def profile(
    profile_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_project_root(Path.cwd())
    cmd = [sys.executable, "tools/profile.py", *profile_args]
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="profile",
    )


def doctor(
    json_output: bool = False,
    verbose: bool = False,
    strict: bool = False,
) -> int:
    root = _find_project_root(Path.cwd())
    checks: list[dict[str, Any]] = []

    def record(name: str, ok: bool, detail: str) -> None:
        checks.append({"name": name, "ok": ok, "detail": detail})

    python_ok = sys.version_info >= (3, 12)
    record(
        "python",
        python_ok,
        f"{sys.version.split()[0]} (requires >=3.12)",
    )

    uv_path = shutil.which("uv")
    record("uv", bool(uv_path), uv_path or "not found")

    cargo_path = shutil.which("cargo")
    record("cargo", bool(cargo_path), cargo_path or "not found")

    rustup_path = shutil.which("rustup")
    record("rustup", bool(rustup_path), rustup_path or "not found")

    cc = os.environ.get("CC", "clang")
    cc_path = shutil.which(cc) or shutil.which("clang")
    record("clang", bool(cc_path), cc_path or "not found")

    zig_path = shutil.which("zig")
    record("zig", bool(zig_path), zig_path or "not found")

    pyproject = root / "pyproject.toml"
    lock_path = root / "uv.lock"
    if pyproject.exists():
        record("uv.lock", lock_path.exists(), str(lock_path))
        if lock_path.exists():
            try:
                if lock_path.stat().st_mtime < pyproject.stat().st_mtime:
                    record(
                        "uv.lock_fresh",
                        False,
                        "uv.lock older than pyproject.toml",
                    )
            except OSError:
                record("uv.lock_fresh", False, "unable to stat uv.lock")

    runtime_lib = root / "target" / "release" / "libmolt_runtime.a"
    record("molt-runtime", runtime_lib.exists(), str(runtime_lib))

    if rustup_path:
        try:
            result = subprocess.run(
                ["rustup", "target", "list", "--installed"],
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError as exc:
            record("rustup-targets", False, f"failed to query: {exc}")
        else:
            targets = result.stdout.split()
            wasm_ok = any(
                target in targets
                for target in ("wasm32-wasip1", "wasm32-unknown-unknown")
            )
            record(
                "wasm-target",
                wasm_ok,
                "wasm32-wasip1 or wasm32-unknown-unknown",
            )

    failures = [check for check in checks if not check["ok"]]
    status = "ok" if not failures else "error"
    if json_output:
        payload = _json_payload(
            "doctor",
            status,
            data={"checks": checks},
        )
        _emit_json(payload, json_output=True)
    else:
        for check in checks:
            marker = "OK" if check["ok"] else "MISSING"
            print(f"{marker}: {check['name']} ({check['detail']})")
    if strict and failures:
        return 1
    return 0


def package(
    artifact: str,
    manifest_path: str,
    output: str | None,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    capabilities: str | list[str] | None = None,
) -> int:
    artifact_path = Path(artifact)
    if not artifact_path.exists():
        return _fail(
            f"Artifact not found: {artifact_path}",
            json_output,
            command="package",
        )
    manifest_file = Path(manifest_path)
    manifest = _load_manifest(manifest_file)
    if manifest is None:
        return _fail(
            f"Failed to load manifest: {manifest_file}",
            json_output,
            command="package",
        )
    errors = _manifest_errors(manifest)
    if errors:
        return _fail(
            "Manifest errors: " + ", ".join(errors),
            json_output,
            command="package",
        )
    if deterministic and manifest.get("deterministic") is not True:
        return _fail(
            "Manifest is not deterministic.",
            json_output,
            command="package",
        )

    capabilities_list = None
    capability_profiles: list[str] = []
    if capabilities:
        parsed, profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="package",
            )
        capabilities_list = parsed
        capability_profiles = profiles
    if capabilities_list is not None:
        required = manifest.get("capabilities", [])
        allowlist = set(capabilities_list) | set(capability_profiles)
        missing = [cap for cap in required if cap not in allowlist]
        if missing:
            return _fail(
                "Capabilities missing from allowlist: " + ", ".join(missing),
                json_output,
                command="package",
            )

    checksum = _sha256_file(artifact_path)
    manifest = dict(manifest)
    manifest["checksum"] = checksum
    name = manifest.get("name", "molt_pkg")
    version = manifest.get("version", "0.0.0")
    target = manifest.get("target", "unknown")

    if output:
        output_path = Path(output)
    else:
        output_path = Path("dist") / f"{name}-{version}-{target}.moltpkg"
    output_path.parent.mkdir(parents=True, exist_ok=True)

    artifact_bytes = artifact_path.read_bytes()
    manifest_bytes = (
        json.dumps(manifest, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    )
    with zipfile.ZipFile(output_path, "w") as zf:
        _write_zip_member(zf, "manifest.json", manifest_bytes)
        _write_zip_member(zf, f"artifact/{artifact_path.name}", artifact_bytes)

    if json_output:
        payload = _json_payload(
            "package",
            "ok",
            data={
                "output": str(output_path),
                "checksum": checksum,
                "deterministic": deterministic,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
            },
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Packaged {output_path}")
        if verbose:
            # TODO(tooling, owner:cli, milestone:TL2): emit SBOM + signature metadata
            # once signing and SBOM generation are implemented.
            print(f"Checksum: {checksum}")
    return 0


def publish(
    package_path: str,
    registry: str,
    dry_run: bool,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    capabilities: str | list[str] | None = None,
) -> int:
    source = Path(package_path)
    if not source.exists():
        return _fail(
            f"Package not found: {source}",
            json_output,
            command="publish",
        )
    if deterministic:
        verify_code = verify(
            package_path,
            None,
            None,
            True,
            False,
            verbose,
            True,
            capabilities,
        )
        if verify_code != 0:
            return verify_code
    registry_path = Path(registry)
    if registry_path.exists() and registry_path.is_dir():
        dest = registry_path / source.name
    elif registry.endswith(os.sep):
        dest = registry_path / source.name
    else:
        dest = registry_path
    if not dry_run:
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, dest)
    if json_output:
        payload = _json_payload(
            "publish",
            "ok",
            data={
                "source": str(source),
                "dest": str(dest),
                "dry_run": dry_run,
                "deterministic": deterministic,
            },
        )
        _emit_json(payload, json_output=True)
    else:
        action = "Would publish" if dry_run else "Published"
        print(f"{action} {source} -> {dest}")
        if verbose:
            # TODO(tooling, owner:cli, milestone:TL2): support registry auth and
            # remote publish flows.
            print(f"Registry: {registry_path}")
    return 0


def verify(
    package_path: str | None,
    manifest_path: str | None,
    artifact_path: str | None,
    require_checksum: bool,
    json_output: bool = False,
    verbose: bool = False,
    require_deterministic: bool = False,
    capabilities: str | list[str] | None = None,
) -> int:
    errors: list[str] = []
    warnings: list[str] = []
    manifest: dict[str, Any] | None = None
    artifact_name = None
    artifact_bytes = None
    capabilities_list = None
    capability_profiles: list[str] = []

    if capabilities:
        parsed, profiles, _source, cap_errors = _parse_capabilities(capabilities)
        if cap_errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(cap_errors),
                json_output,
                command="verify",
            )
        capabilities_list = parsed
        capability_profiles = profiles

    if package_path:
        pkg_path = Path(package_path)
        if not pkg_path.exists():
            return _fail(
                f"Package not found: {pkg_path}",
                json_output,
                command="verify",
            )
        try:
            with zipfile.ZipFile(pkg_path) as zf:
                try:
                    manifest_bytes = zf.read("manifest.json")
                except KeyError:
                    errors.append("manifest.json not found in package")
                else:
                    manifest = json.loads(manifest_bytes.decode("utf-8"))
                artifact_entries = [
                    name for name in zf.namelist() if name.startswith("artifact/")
                ]
                if len(artifact_entries) == 1:
                    artifact_name = artifact_entries[0]
                    artifact_bytes = zf.read(artifact_name)
                elif not artifact_entries:
                    errors.append("artifact/* not found in package")
                else:
                    errors.append("multiple artifact entries in package")
        except (OSError, zipfile.BadZipFile) as exc:
            return _fail(
                f"Failed to read package: {exc}",
                json_output,
                command="verify",
            )
    else:
        if not manifest_path or not artifact_path:
            return _fail(
                "Provide --package or both --manifest and --artifact.",
                json_output,
                command="verify",
            )
        manifest = _load_manifest(Path(manifest_path))
        if manifest is None:
            errors.append("failed to load manifest")
        artifact_file = Path(artifact_path)
        if not artifact_file.exists():
            errors.append("artifact not found")
        else:
            artifact_name = artifact_file.name
            artifact_bytes = artifact_file.read_bytes()

    if manifest is not None:
        errors.extend(_manifest_errors(manifest))
        checksum = manifest.get("checksum")
        if checksum and artifact_bytes is not None:
            actual = hashlib.sha256(artifact_bytes).hexdigest()
            if actual != checksum:
                errors.append("checksum mismatch")
        elif require_checksum:
            errors.append("checksum missing")
        elif not checksum:
            warnings.append("checksum missing")
        if require_deterministic and manifest.get("deterministic") is not True:
            errors.append("manifest is not deterministic")
        if capabilities_list is not None:
            required = manifest.get("capabilities", [])
            allowlist = set(capabilities_list) | set(capability_profiles)
            missing = [cap for cap in required if cap not in allowlist]
            if missing:
                errors.append(
                    "capabilities missing from allowlist: " + ", ".join(missing)
                )
        # TODO(tooling, owner:cli, milestone:TL2): enforce ABI compatibility and
        # schema versioning when the package ABI stabilizes.

    status = "ok" if not errors else "error"
    if json_output:
        payload = _json_payload(
            "verify",
            status,
            data={
                "artifact": artifact_name,
                "deterministic": require_deterministic,
                "capability_profiles": capability_profiles,
            },
            warnings=warnings,
            errors=errors,
        )
        _emit_json(payload, json_output=True)
    else:
        for err in errors:
            print(f"ERROR: {err}")
        for warn in warnings:
            print(f"WARN: {warn}")
        if not errors and verbose:
            print("Verification passed")
    return 0 if not errors else 1


def _summarize_tiers(rows: list[dict[str, Any]]) -> dict[str, int]:
    summary: dict[str, int] = {"Tier A": 0, "Tier B": 0, "Tier C": 0}
    for row in rows:
        tier = row.get("tier")
        if tier in summary:
            summary[tier] += 1
    return summary


def deps(include_dev: bool, json_output: bool = False, verbose: bool = False) -> int:
    pyproject = _load_toml(Path("pyproject.toml"))
    lock = _load_toml(Path("uv.lock"))
    deps = _collect_deps(pyproject, include_dev=include_dev)
    packages = _lock_packages(lock)
    allow = _dep_allowlists(pyproject)

    rows: list[dict[str, Any]] = []
    for dep in deps:
        key = _normalize_name(dep)
        pkg = packages.get(key)
        version = pkg.get("version") if pkg else None
        tier, reason = _classify_tier(dep, pkg, allow)
        rows.append({"name": dep, "version": version, "tier": tier, "reason": reason})

    if json_output:
        data: dict[str, Any] = {"dependencies": rows}
        if verbose:
            data["summary"] = _summarize_tiers(rows)
        payload = _json_payload("deps", "ok", data=data)
        _emit_json(payload, json_output)
        return 0

    for row in rows:
        version = row["version"] or "missing"
        print(f"{row['name']} {version} {row['tier']} {row['reason']}")
    if verbose:
        summary = _summarize_tiers(rows)
        print(
            "Summary: "
            + ", ".join(f"{tier}={count}" for tier, count in summary.items())
        )
    return 0


def vendor(
    include_dev: bool,
    json_output: bool = False,
    verbose: bool = False,
    output: str | None = None,
    dry_run: bool = False,
    allow_non_tier_a: bool = False,
    extras: list[str] | None = None,
) -> int:
    root = _find_project_root(Path.cwd())
    pyproject = _load_toml(root / "pyproject.toml")
    lock = _load_toml(root / "uv.lock")
    extras_set: set[str] = set()
    for extra in extras or []:
        for token in re.split(r"[,\s]+", extra):
            if token:
                extras_set.add(token)
    deps, root_extras, skipped_root = _collect_dep_specs(
        pyproject,
        include_dev=include_dev,
        extras=extras_set,
    )
    env = _marker_environment()
    packages, deps_graph, skipped = _lock_package_graph(
        lock,
        env=env,
        selected_extras=root_extras,
    )
    allow = _dep_allowlists(pyproject)

    root_names = deps
    closure, missing = _resolve_dependency_closure(root_names, deps_graph)
    vendor_list: list[dict[str, Any]] = []
    blockers: list[dict[str, Any]] = []
    for name in closure:
        pkg = packages.get(name)
        display = pkg.get("name", name) if pkg else name
        tier, reason = _classify_tier(display, pkg, allow)
        version = pkg.get("version") if pkg else None
        entry = {
            "name": display,
            "version": version,
            "tier": tier,
            "reason": reason,
        }
        if tier == "Tier A":
            vendor_list.append(entry)
        else:
            blockers.append(entry)

    if missing:
        blockers.append(
            {
                "name": ",".join(missing),
                "version": None,
                "tier": "Unknown",
                "reason": "missing from uv.lock",
            }
        )

    if blockers and not allow_non_tier_a:
        if json_output:
            payload = _json_payload(
                "vendor",
                "error",
                data={
                    "vendor": vendor_list,
                    "blockers": blockers,
                    "missing": missing,
                    "extras": sorted(extras_set),
                    "skipped": skipped,
                    "skipped_root": skipped_root,
                },
                errors=["vendoring blocked by non-Tier A dependencies"],
            )
            _emit_json(payload, json_output=True)
            return 2
        print("Vendoring blocked by non-Tier A dependencies:")
        for entry in blockers:
            version = entry["version"] or "missing"
            print(f"- {entry['name']} {version} {entry['tier']} {entry['reason']}")
        return 2

    output_dir = Path(output) if output else Path("vendor")
    package_dir = output_dir / "packages"
    local_dir = output_dir / "local"
    manifest: dict[str, Any] = {
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "root": str(root),
        "include_dev": include_dev,
        "extras": sorted(extras_set),
        "packages": [],
        "blockers": blockers,
        "missing": missing,
        "skipped": skipped,
        "skipped_root": skipped_root,
    }

    if not dry_run:
        package_dir.mkdir(parents=True, exist_ok=True)
        local_dir.mkdir(parents=True, exist_ok=True)

    for entry in vendor_list:
        pkg = packages.get(_normalize_name(entry["name"]))
        if not pkg:
            continue
        source = pkg.get("source", {})
        if source.get("path"):
            src_path = Path(source["path"])
            if not src_path.is_absolute():
                src_path = (root / src_path).resolve()
            dest = local_dir / entry["name"]
            if not dry_run:
                if dest.exists():
                    shutil.rmtree(dest)
                if src_path.is_dir():
                    shutil.copytree(src_path, dest)
                else:
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(src_path, dest)
            manifest["packages"].append(
                {
                    **entry,
                    "source": "path",
                    "path": str(src_path),
                }
            )
            continue
        if source.get("git"):
            # TODO(tooling, owner:cli, milestone:TL2): support vendoring git sources
            # with pinned revisions and integrity metadata.
            blockers.append(
                {
                    **entry,
                    "tier": "Tier A",
                    "reason": "git source not supported for vendoring",
                }
            )
            continue
        picked = _pick_vendor_artifact(pkg)
        if picked is None:
            blockers.append(
                {**entry, "tier": "Tier A", "reason": "no artifact in uv.lock"}
            )
            continue
        kind, artifact = picked
        url = artifact.get("url", "")
        hash_value = artifact.get("hash", "")
        filename = Path(url).name if url else f"{entry['name']}-{entry['version']}"
        dest = package_dir / filename
        if not dry_run:
            try:
                data = _download_artifact(url, hash_value)
            except Exception as exc:
                return _fail(
                    f"Failed to download {url}: {exc}",
                    json_output,
                    command="vendor",
                )
            dest.write_bytes(data)
        manifest["packages"].append(
            {
                **entry,
                "source": kind,
                "url": url,
                "hash": hash_value,
                "file": str(dest),
            }
        )

    if not dry_run:
        manifest_path = output_dir / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")

    if json_output:
        data: dict[str, Any] = {
            "vendor": vendor_list,
            "blockers": blockers,
            "missing": missing,
            "output": str(output_dir),
            "dry_run": dry_run,
            "extras": sorted(extras_set),
            "skipped": skipped,
            "skipped_root": skipped_root,
        }
        if verbose:
            data["count"] = len(vendor_list)
        payload = _json_payload("vendor", "ok", data=data)
        _emit_json(payload, json_output=True)
        return 0

    banner = "Vendoring plan (Tier A)" if dry_run else "Vendoring Tier A packages"
    print(f"{banner}:")
    for entry in vendor_list:
        version = entry["version"] or "missing"
        print(f"- {entry['name']} {version}")
    if blockers:
        print("Blockers:")
        for entry in blockers:
            version = entry["version"] or "missing"
            print(f"- {entry['name']} {version} {entry['tier']} {entry['reason']}")
    if verbose:
        print(f"Total Tier A packages: {len(vendor_list)}")
        print(f"Output: {output_dir}")
    return 0


def clean(
    json_output: bool = False,
    verbose: bool = False,
    cache: bool = True,
    artifacts: bool = True,
    cargo_target: bool = False,
) -> int:
    root = _find_project_root(Path.cwd())
    removed: list[str] = []
    missing: list[str] = []
    if cache:
        cache_root = _default_molt_cache()
        if cache_root.exists():
            shutil.rmtree(cache_root)
            removed.append(str(cache_root))
        else:
            missing.append(str(cache_root))
        legacy_root = root / ".molt"
        # TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): remove legacy .molt cleanup once one-time MOLT_HOME/MOLT_CACHE migration is complete.
        if legacy_root.exists():
            shutil.rmtree(legacy_root)
            removed.append(str(legacy_root))
        else:
            missing.append(str(legacy_root))
    if artifacts:
        build_root = _default_molt_home() / "build"
        if build_root.exists():
            shutil.rmtree(build_root)
            removed.append(str(build_root))
        else:
            missing.append(str(build_root))
        # TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): remove legacy output artifact cleanup after out-dir defaults land.
        for name in ("output.o", "output.wasm", "main_stub.c"):
            path = root / name
            if path.exists():
                path.unlink()
                removed.append(str(path))
            else:
                missing.append(str(path))
    if cargo_target:
        cargo_root = root / "target"
        if cargo_root.exists():
            shutil.rmtree(cargo_root)
            removed.append(str(cargo_root))
        else:
            missing.append(str(cargo_root))
    if json_output:
        data: dict[str, Any] = {"removed": removed}
        if verbose:
            data["missing"] = missing
        payload = _json_payload("clean", "ok", data=data)
        _emit_json(payload, json_output=True)
    else:
        if removed:
            print("Removed:")
            for path in removed:
                print(f"- {path}")
        if verbose and missing:
            print("Missing:")
            for path in missing:
                print(f"- {path}")
    return 0


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
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    caps_cfg = _resolve_capabilities_config(config)
    data: dict[str, Any] = {
        "root": str(config_root),
        "sources": {
            "molt_toml": str(molt_toml) if molt_toml.exists() else None,
            "pyproject": str(pyproject) if pyproject.exists() else None,
        },
        "build": build_cfg,
        "run": run_cfg,
        "test": test_cfg,
        "diff": diff_cfg,
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
    if caps_cfg:
        print(f"Capabilities: {', '.join(caps_cfg)}")
    else:
        print("Capabilities: none")
    if verbose:
        print("Merged config:")
        print(json.dumps(config, indent=2))
    return 0


def _completion_script(shell: str) -> str:
    commands = [
        "build",
        "check",
        "run",
        "test",
        "diff",
        "bench",
        "profile",
        "lint",
        "doctor",
        "package",
        "publish",
        "verify",
        "deps",
        "vendor",
        "clean",
        "config",
        "completion",
    ]
    options = {
        "build": [
            "--target",
            "--codec",
            "--type-hints",
            "--fallback",
            "--type-facts",
            "--output",
            "--out-dir",
            "--emit",
            "--emit-ir",
            "--profile",
            "--deterministic",
            "--no-deterministic",
            "--trusted",
            "--no-trusted",
            "--capabilities",
            "--cache",
            "--no-cache",
            "--cache-dir",
            "--cache-report",
            "--rebuild",
            "--json",
            "--verbose",
        ],
        "check": ["--output", "--strict", "--json", "--verbose"],
        "run": [
            "--python",
            "--python-version",
            "--no-shims",
            "--compiled",
            "--build-arg",
            "--rebuild",
            "--compiled-args",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "test": [
            "--suite",
            "--python-version",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "diff": [
            "--python-version",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "bench": ["--wasm", "--json", "--verbose"],
        "profile": ["--json", "--verbose"],
        "lint": ["--json", "--verbose"],
        "doctor": ["--strict", "--json", "--verbose"],
        "package": [
            "--output",
            "--deterministic",
            "--no-deterministic",
            "--capabilities",
            "--json",
            "--verbose",
        ],
        "publish": [
            "--registry",
            "--dry-run",
            "--deterministic",
            "--no-deterministic",
            "--capabilities",
            "--json",
            "--verbose",
        ],
        "verify": [
            "--package",
            "--manifest",
            "--artifact",
            "--require-checksum",
            "--require-deterministic",
            "--capabilities",
            "--json",
            "--verbose",
        ],
        "deps": ["--include-dev", "--json", "--verbose"],
        "vendor": [
            "--include-dev",
            "--output",
            "--dry-run",
            "--allow-non-tier-a",
            "--extras",
            "--json",
            "--verbose",
        ],
        "clean": [
            "--cache",
            "--no-cache",
            "--artifacts",
            "--no-artifacts",
            "--json",
            "--verbose",
        ],
        "config": ["--file", "--json", "--verbose"],
        "completion": ["--shell", "--json", "--verbose"],
    }
    if shell == "bash":
        lines = [
            "_molt_complete() {",
            "  local cur prev",
            "  COMPREPLY=()",
            '  cur="${COMP_WORDS[COMP_CWORD]}"',
            '  prev="${COMP_WORDS[COMP_CWORD-1]}"',
            "  if [[ ${COMP_CWORD} -eq 1 ]]; then",
            f'    COMPREPLY=( $(compgen -W "{" ".join(commands)}" -- "$cur") )',
            "    return 0",
            "  fi",
            '  case "${COMP_WORDS[1]}" in',
        ]
        for cmd in commands:
            opts = " ".join(options.get(cmd, []))
            lines.append(f'    {cmd}) opts="{opts}" ;;')
        lines.extend(
            [
                '    *) opts="" ;;',
                "  esac",
                '  COMPREPLY=( $(compgen -W "$opts" -- "$cur") )',
                "}",
                "complete -F _molt_complete molt",
            ]
        )
        return "\n".join(lines) + "\n"
    if shell == "zsh":
        lines = [
            "#compdef molt",
            "_molt() {",
            "  local -a commands",
            f"  commands=({' '.join(commands)})",
            "  if (( CURRENT == 2 )); then",
            "    compadd $commands",
            "    return",
            "  fi",
            "  local -a opts",
            "  case $words[2] in",
        ]
        for cmd in commands:
            opts = " ".join(options.get(cmd, []))
            lines.append(f"    {cmd}) opts=({opts}) ;;")
        lines.extend(
            [
                "    *) opts=() ;;",
                "  esac",
                "  compadd $opts",
                "}",
                "compdef _molt molt",
            ]
        )
        return "\n".join(lines) + "\n"
    if shell == "fish":
        lines = [
            f"complete -c molt -f -n '__fish_use_subcommand' -a \"{' '.join(commands)}\"",
        ]
        for cmd in commands:
            for opt in options.get(cmd, []):
                opt_name = opt.lstrip("-")
                lines.append(
                    f"complete -c molt -n '__fish_seen_subcommand_from {cmd}' -l {opt_name}"
                )
        return "\n".join(lines) + "\n"
    raise ValueError(f"Unsupported shell: {shell}")


def completion(shell: str, json_output: bool = False, verbose: bool = False) -> int:
    try:
        script = _completion_script(shell)
    except ValueError as exc:
        return _fail(str(exc), json_output, command="completion")
    if json_output:
        payload = _json_payload(
            "completion",
            "ok",
            data={"shell": shell, "script": script},
        )
        _emit_json(payload, json_output=True)
    else:
        print(script, end="")
    return 0


def _strip_leading_double_dash(args: list[str]) -> list[str]:
    if args and args[0] == "--":
        return args[1:]
    return args


def _extract_output_arg(args: list[str]) -> Path | None:
    for idx, arg in enumerate(args):
        if arg == "--output" and idx + 1 < len(args):
            return Path(args[idx + 1])
        if arg.startswith("--output="):
            return Path(arg.split("=", 1)[1])
    return None


def _extract_out_dir_arg(args: list[str]) -> Path | None:
    for idx, arg in enumerate(args):
        if arg == "--out-dir" and idx + 1 < len(args):
            return Path(args[idx + 1])
        if arg.startswith("--out-dir="):
            return Path(arg.split("=", 1)[1])
    return None


def _extract_emit_arg(args: list[str]) -> str | None:
    for idx, arg in enumerate(args):
        if arg == "--emit" and idx + 1 < len(args):
            return args[idx + 1]
        if arg.startswith("--emit="):
            return arg.split("=", 1)[1]
    return None


def _build_args_has_cache_flag(args: list[str]) -> bool:
    for arg in args:
        if arg in {"--cache", "--no-cache", "--rebuild"}:
            return True
    return False


def _build_args_has_trusted_flag(args: list[str]) -> bool:
    for arg in args:
        if arg in {"--trusted", "--no-trusted"}:
            return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser(prog="molt")
    subparsers = parser.add_subparsers(dest="command", required=True)

    build_parser = subparsers.add_parser("build", help="Compile a Python file")
    build_parser.add_argument("file", help="Path to Python source")
    build_parser.add_argument(
        "--target",
        default=None,
        help="Target backend: native, wasm, or a target triple.",
    )
    build_parser.add_argument(
        "--codec",
        choices=["msgpack", "cbor", "json"],
        default=None,
        help="Structured codec for parse calls (default from config or msgpack).",
    )
    build_parser.add_argument(
        "--type-hints",
        choices=["ignore", "trust", "check"],
        default=None,
        help="Apply type annotations to guide lowering and specialization.",
    )
    build_parser.add_argument(
        "--fallback",
        choices=["error", "bridge"],
        default=None,
        help="Fallback policy for unsupported constructs.",
    )
    build_parser.add_argument(
        "--type-facts",
        help="Path to type facts JSON from `molt check`.",
    )
    build_parser.add_argument(
        "--output",
        help=(
            "Output path for the native binary or wasm artifact "
            "(relative to --out-dir when set, otherwise project root)."
        ),
    )
    build_parser.add_argument(
        "--out-dir",
        help=(
            "Output directory for build artifacts (output.o/main_stub.c/output.wasm). "
            "When set, native binaries default here too. Defaults to "
            "MOLT_HOME/build/<entry> for artifacts and MOLT_BIN for native binaries."
        ),
    )
    build_parser.add_argument(
        "--emit",
        choices=["bin", "obj", "wasm"],
        default=None,
        help="Select which artifact to emit (native: bin/obj, wasm: wasm).",
    )
    build_parser.add_argument(
        "--linked",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Emit a linked wasm artifact (output_linked.wasm) alongside output.wasm.",
    )
    build_parser.add_argument(
        "--linked-output",
        help=(
            "Output path for the linked wasm artifact "
            "(relative to --out-dir when set, otherwise project root)."
        ),
    )
    build_parser.add_argument(
        "--require-linked",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require linked wasm output for wasm targets (fails if linking is unavailable).",
    )
    build_parser.add_argument(
        "--emit-ir",
        help="Write the lowered IR JSON to a file path.",
    )
    build_parser.add_argument(
        "--profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for backend/runtime (default: release).",
    )
    build_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    build_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments (native only).",
    )
    build_parser.add_argument(
        "--cache",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Enable build cache under MOLT_CACHE (defaults to the OS cache).",
    )
    build_parser.add_argument(
        "--cache-dir",
        help="Override the build cache directory (default: MOLT_CACHE).",
    )
    build_parser.add_argument(
        "--cache-report",
        action="store_true",
        help="Print cache hit/miss details even without --verbose.",
    )
    build_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable the build cache (alias for --no-cache).",
    )
    build_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    build_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    build_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    check_parser = subparsers.add_parser(
        "check", help="Generate a type facts artifact (ty-backed when available)"
    )
    check_parser.add_argument("path", help="Python file or package directory")
    check_parser.add_argument(
        "--output",
        default="type_facts.json",
        help="Output path for type facts JSON.",
    )
    check_parser.add_argument(
        "--strict",
        action="store_true",
        help="Mark facts as trusted (strict tier).",
    )
    check_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    check_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    run_parser = subparsers.add_parser(
        "run", help="Run Python via CPython with Molt shims"
    )
    run_parser.add_argument("file", help="Path to Python source")
    run_parser.add_argument(
        "--python",
        help="Python interpreter (path) or version (e.g. 3.12).",
    )
    run_parser.add_argument(
        "--python-version",
        help="Python version alias (e.g. 3.12).",
    )
    run_parser.add_argument(
        "--no-shims",
        action="store_true",
        help="Disable Molt shims and run raw CPython.",
    )
    run_parser.add_argument(
        "--compiled",
        action="store_true",
        help="Compile with Molt and run the native binary instead of CPython.",
    )
    run_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build` when using --compiled.",
    )
    run_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable build cache when using --compiled.",
    )
    run_parser.add_argument(
        "--compiled-args",
        action="store_true",
        help="Pass argv through to compiled binaries (runtime support pending).",
    )
    run_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    run_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    run_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    run_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    test_parser = subparsers.add_parser("test", help="Run Molt test suites")
    test_parser.add_argument(
        "--suite",
        choices=["dev", "diff", "pytest"],
        default="dev",
        help="Test suite to run.",
    )
    test_parser.add_argument(
        "--python-version",
        help="Python version for diff suite (e.g. 3.13).",
    )
    test_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    test_parser.add_argument("path", nargs="?", help="Optional test path.")
    test_parser.add_argument(
        "pytest_args",
        nargs=argparse.REMAINDER,
        help="Extra pytest args when --suite pytest (use -- to separate).",
    )
    test_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    test_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    diff_parser = subparsers.add_parser(
        "diff", help="Run differential tests against CPython"
    )
    diff_parser.add_argument("path", nargs="?", help="File or directory to test.")
    diff_parser.add_argument(
        "--python-version", help="Python version to test against (e.g. 3.13)."
    )
    diff_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    diff_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    diff_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    bench_parser = subparsers.add_parser("bench", help="Run benchmark suites")
    bench_parser.add_argument(
        "--wasm", action="store_true", help="Use the WASM bench harness."
    )
    bench_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    bench_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    bench_parser.add_argument(
        "bench_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the bench tool (use -- to separate).",
    )

    profile_parser = subparsers.add_parser("profile", help="Profile benchmarks")
    profile_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    profile_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    profile_parser.add_argument(
        "profile_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the profile tool (use -- to separate).",
    )

    lint_parser = subparsers.add_parser("lint", help="Run linting checks")
    lint_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    lint_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    doctor_parser = subparsers.add_parser("doctor", help="Check toolchain setup")
    doctor_parser.add_argument(
        "--strict",
        action="store_true",
        help="Return non-zero exit on missing requirements.",
    )
    doctor_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    doctor_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    package_parser = subparsers.add_parser(
        "package", help="Bundle a Molt package artifact"
    )
    package_parser.add_argument("artifact", help="Path to the package artifact.")
    package_parser.add_argument(
        "manifest",
        help="Path to manifest JSON (fields per docs/spec/0018_MOLT_PACKAGE_ABI.md).",
    )
    package_parser.add_argument(
        "--output",
        help="Output .moltpkg path (default dist/<name>-<version>-<target>.moltpkg).",
    )
    package_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic package metadata.",
    )
    package_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    package_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    package_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    publish_parser = subparsers.add_parser(
        "publish", help="Publish a Molt package to a local registry path"
    )
    publish_parser.add_argument("package", help="Path to the .moltpkg file.")
    publish_parser.add_argument(
        "--registry",
        default="dist/registry",
        help="Registry directory or file path.",
    )
    publish_parser.add_argument(
        "--dry-run", action="store_true", help="Print the publish plan only."
    )
    publish_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package determinism before publishing.",
    )
    publish_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    publish_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    publish_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    verify_parser = subparsers.add_parser(
        "verify", help="Verify a Molt package manifest and checksum"
    )
    verify_parser.add_argument(
        "--package",
        help="Path to the .moltpkg archive (alternative to --manifest/--artifact).",
    )
    verify_parser.add_argument("--manifest", help="Manifest JSON path.")
    verify_parser.add_argument("--artifact", help="Artifact path.")
    verify_parser.add_argument(
        "--require-checksum",
        action="store_true",
        help="Fail when checksum is missing.",
    )
    verify_parser.add_argument(
        "--require-deterministic",
        action="store_true",
        help="Fail when manifest is not deterministic.",
    )
    verify_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    verify_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    verify_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    deps_parser = subparsers.add_parser(
        "deps", help="Show dependency compatibility info"
    )
    deps_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    deps_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    deps_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    vendor_parser = subparsers.add_parser(
        "vendor", help="Vendor pure Python dependencies"
    )
    vendor_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    vendor_parser.add_argument(
        "--output",
        help="Output directory for vendored artifacts (default: vendor).",
    )
    vendor_parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show vendoring plan without downloading artifacts.",
    )
    vendor_parser.add_argument(
        "--allow-non-tier-a",
        action="store_true",
        help="Proceed even if non-Tier A dependencies are present.",
    )
    vendor_parser.add_argument(
        "--extras",
        action="append",
        help="Extras to include from project optional-dependencies.",
    )
    vendor_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    vendor_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    clean_parser = subparsers.add_parser(
        "clean", help="Remove Molt cache and transient build artifacts"
    )
    clean_parser.add_argument(
        "--cache",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Remove build caches under MOLT_CACHE (and legacy .molt caches).",
    )
    clean_parser.add_argument(
        "--artifacts",
        action=argparse.BooleanOptionalAction,
        default=True,
        help=(
            "Remove build artifacts under MOLT_HOME/build "
            "(and legacy output.o/output.wasm/main_stub.c)."
        ),
    )
    clean_parser.add_argument(
        "--cargo-target",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Remove Cargo target/ build artifacts in the repo root.",
    )
    clean_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    clean_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    config_parser = subparsers.add_parser(
        "config", help="Show Molt configuration defaults"
    )
    config_parser.add_argument(
        "--file",
        help="Resolve project root from a source file path.",
    )
    config_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    config_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    completion_parser = subparsers.add_parser(
        "completion", help="Generate shell completion scripts"
    )
    completion_parser.add_argument(
        "--shell",
        choices=["bash", "zsh", "fish"],
        default="bash",
        help="Shell type to emit.",
    )
    completion_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    completion_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    args = parser.parse_args()

    config_root = _find_project_root(Path.cwd())
    if getattr(args, "file", None):
        try:
            config_root = _find_project_root(Path(args.file).resolve())
        except OSError:
            config_root = _find_project_root(Path.cwd())
    config = _load_molt_config(config_root)
    build_cfg = _resolve_build_config(config)
    run_cfg = _resolve_command_config(config, "run")
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    cfg_capabilities = _resolve_capabilities_config(config)

    if args.command == "build":
        target = args.target or build_cfg.get("target") or "native"
        codec = args.codec or build_cfg.get("codec") or "msgpack"
        type_hints = args.type_hints or build_cfg.get("type_hints") or "ignore"
        fallback = args.fallback or build_cfg.get("fallback") or "error"
        output = args.output or build_cfg.get("output")
        out_dir = args.out_dir or build_cfg.get("out_dir") or build_cfg.get("out-dir")
        emit = args.emit or build_cfg.get("emit")
        emit_ir = args.emit_ir or build_cfg.get("emit_ir") or build_cfg.get("emit-ir")
        build_profile = (
            args.profile
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "release"
        )
        linked_output_path = (
            args.linked_output
            or build_cfg.get("linked_output")
            or build_cfg.get("linked-output")
        )
        require_linked = args.require_linked
        if require_linked is None:
            require_linked = _coerce_bool(
                build_cfg.get("require_linked") or build_cfg.get("require-linked"),
                False,
            )
        type_facts = args.type_facts or build_cfg.get("type_facts")
        deterministic = (
            args.deterministic
            if args.deterministic is not None
            else _coerce_bool(build_cfg.get("deterministic"), True)
        )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(build_cfg.get("trusted"), False)
        linked = args.linked
        if linked is None:
            linked = _coerce_bool(build_cfg.get("linked"), False)
        cache = (
            args.cache
            if args.cache is not None
            else _coerce_bool(build_cfg.get("cache"), True)
        )
        if args.rebuild:
            cache = False
        cache_dir = (
            args.cache_dir or build_cfg.get("cache_dir") or build_cfg.get("cache-dir")
        )
        cache_report = args.cache_report or _coerce_bool(
            build_cfg.get("cache_report") or build_cfg.get("cache-report"), False
        )
        capabilities = (
            args.capabilities or build_cfg.get("capabilities") or cfg_capabilities
        )
        return build(
            args.file,
            target,
            codec,
            type_hints,
            fallback,
            type_facts,
            output,
            args.json,
            args.verbose,
            deterministic,
            trusted,
            capabilities,
            cache,
            cache_dir,
            cache_report,
            emit_ir,
            emit,
            out_dir,
            build_profile,
            linked,
            linked_output_path,
            require_linked,
        )
    if args.command == "check":
        return check(args.path, args.output, args.strict, args.json, args.verbose)
    if args.command == "run":
        python_exe = args.python or args.python_version
        build_args = _strip_leading_double_dash(args.build_arg)
        if args.rebuild and not _build_args_has_cache_flag(build_args):
            build_args.append("--no-cache")
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(run_cfg.get("trusted"), False)
        return run_script(
            args.file,
            python_exe,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            args.no_shims,
            args.compiled,
            args.compiled_args,
            trusted,
            build_args,
        )
    if args.command == "test":
        pytest_args = _strip_leading_double_dash(args.pytest_args)
        if args.suite == "dev" and (args.path or pytest_args) and args.verbose:
            print("Ignoring extra args for suite=dev.")
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(test_cfg.get("trusted"), False)
        return test(
            args.suite,
            args.path,
            args.python_version,
            pytest_args,
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "diff":
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(diff_cfg.get("trusted"), False)
        return diff(
            args.path,
            args.python_version,
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "bench":
        return bench(
            args.wasm,
            _strip_leading_double_dash(args.bench_args),
            args.json,
            args.verbose,
        )
    if args.command == "profile":
        return profile(
            _strip_leading_double_dash(args.profile_args),
            args.json,
            args.verbose,
        )
    if args.command == "lint":
        return lint(args.json, args.verbose)
    if args.command == "doctor":
        return doctor(args.json, args.verbose, args.strict)
    if args.command == "package":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        capabilities = args.capabilities or cfg_capabilities
        return package(
            args.artifact,
            args.manifest,
            args.output,
            args.json,
            args.verbose,
            deterministic,
            capabilities,
        )
    if args.command == "publish":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        capabilities = args.capabilities or cfg_capabilities
        return publish(
            args.package,
            args.registry,
            args.dry_run,
            args.json,
            args.verbose,
            deterministic,
            capabilities,
        )
    if args.command == "verify":
        return verify(
            args.package,
            args.manifest,
            args.artifact,
            args.require_checksum,
            args.json,
            args.verbose,
            args.require_deterministic,
            args.capabilities or cfg_capabilities,
        )
    if args.command == "deps":
        return deps(args.include_dev, args.json, args.verbose)
    if args.command == "vendor":
        return vendor(
            args.include_dev,
            args.json,
            args.verbose,
            args.output,
            args.dry_run,
            args.allow_non_tier_a,
            args.extras,
        )
    if args.command == "clean":
        return clean(
            args.json, args.verbose, args.cache, args.artifacts, args.cargo_target
        )
    if args.command == "config":
        return show_config(config_root, config, args.json, args.verbose)
    if args.command == "completion":
        return completion(args.shell, args.json, args.verbose)

    return 2


def _load_toml(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    return tomllib.loads(path.read_text())


def _normalize_name(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def _marker_environment() -> dict[str, str]:
    version = sys.version_info
    return {
        "python_version": f"{version.major}.{version.minor}",
        "python_full_version": f"{version.major}.{version.minor}.{version.micro}",
        "sys_platform": sys.platform,
        "platform_system": platform.system(),
        "platform_machine": platform.machine(),
        "platform_release": platform.release(),
        "platform_version": platform.version(),
        "implementation_name": sys.implementation.name,
        "implementation_version": sys.implementation.version.__str__(),
    }


def _parse_requirement(spec: str) -> tuple[str, set[str], str | None]:
    head, *marker_parts = spec.split(";", 1)
    marker = marker_parts[0].strip() if marker_parts else None
    head = head.strip()
    head = head.split("@", 1)[0].strip()
    match = re.match(r"^([A-Za-z0-9_.-]+)(?:\[([^\]]+)\])?", head)
    if not match:
        return "", set(), marker
    name = match.group(1)
    extras_raw = match.group(2) or ""
    extras = {extra.strip() for extra in extras_raw.split(",") if extra.strip()}
    return name, extras, marker


def _version_key(value: str) -> tuple[int, ...]:
    parts = []
    for chunk in re.split(r"[.+-]", value):
        if not chunk:
            continue
        if chunk.isdigit():
            parts.append(int(chunk))
        else:
            parts.append(0)
    return tuple(parts)


def _eval_marker_value(node: ast.AST, env: dict[str, str]) -> tuple[Any, str]:
    if isinstance(node, ast.Name):
        value = env.get(node.id, "")
        kind = (
            "version" if node.id in {"python_version", "python_full_version"} else "str"
        )
        return value, kind
    if isinstance(node, ast.Constant):
        return node.value, "str"
    raise ValueError("Unsupported marker value")


def _eval_marker(node: ast.AST, env: dict[str, str]) -> bool:
    if isinstance(node, ast.Expression):
        return _eval_marker(node.body, env)
    if isinstance(node, ast.BoolOp):
        values = [_eval_marker(value, env) for value in node.values]
        if isinstance(node.op, ast.And):
            return all(values)
        if isinstance(node.op, ast.Or):
            return any(values)
        raise ValueError("Unsupported boolean op")
    if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
        return not _eval_marker(node.operand, env)
    if isinstance(node, ast.Compare):
        left_val, left_kind = _eval_marker_value(node.left, env)
        for op, comparator in zip(node.ops, node.comparators):
            right_val, right_kind = _eval_marker_value(comparator, env)
            use_version = left_kind == "version" or right_kind == "version"
            if use_version:
                left_cmp = _version_key(str(left_val))
                right_cmp = _version_key(str(right_val))
            else:
                left_cmp = left_val
                right_cmp = right_val
            if isinstance(op, ast.Eq):
                ok = left_cmp == right_cmp
            elif isinstance(op, ast.NotEq):
                ok = left_cmp != right_cmp
            elif isinstance(op, ast.Lt):
                ok = left_cmp < right_cmp
            elif isinstance(op, ast.LtE):
                ok = left_cmp <= right_cmp
            elif isinstance(op, ast.Gt):
                ok = left_cmp > right_cmp
            elif isinstance(op, ast.GtE):
                ok = left_cmp >= right_cmp
            else:
                raise ValueError("Unsupported comparison op")
            if not ok:
                return False
            left_val, left_kind = right_val, right_kind
        return True
    raise ValueError("Unsupported marker expression")


def _marker_satisfied(
    marker: str,
    env: dict[str, str],
    extras: set[str],
) -> bool:
    try:
        tree = ast.parse(marker, mode="eval")
    except SyntaxError:
        return False
    # TODO(tooling, owner:cli, milestone:TL2): replace with a full PEP 508 marker
    # parser/evaluator (packaging markers) to match pip/uv behavior.
    if "extra" in marker:
        if extras:
            return any(_eval_marker(tree, {**env, "extra": extra}) for extra in extras)
        return _eval_marker(tree, {**env, "extra": ""})
    return _eval_marker(tree, env)


def _collect_dep_specs(
    pyproject: dict[str, Any],
    include_dev: bool,
    extras: set[str] | None = None,
) -> tuple[list[str], dict[str, set[str]], list[str]]:
    deps: list[str] = []
    root_extras: dict[str, set[str]] = {}
    skipped: list[str] = []
    entries: list[str] = []
    entries.extend(pyproject.get("project", {}).get("dependencies", []))
    if include_dev:
        entries.extend(pyproject.get("dependency-groups", {}).get("dev", []))
    extras = extras or set()
    optional = pyproject.get("project", {}).get("optional-dependencies", {})
    for extra in extras:
        entries.extend(optional.get(extra, []))
    env = _marker_environment()
    for entry in entries:
        name, req_extras, marker = _parse_requirement(entry)
        if not name:
            continue
        if marker and not _marker_satisfied(marker, env, extras):
            skipped.append(entry)
            continue
        norm = _normalize_name(name)
        deps.append(norm)
        if req_extras:
            root_extras.setdefault(norm, set()).update(req_extras)
    return deps, root_extras, skipped


def _collect_deps(pyproject: dict[str, Any], include_dev: bool) -> list[str]:
    deps: list[str] = []
    deps.extend(pyproject.get("project", {}).get("dependencies", []))
    if include_dev:
        deps.extend(pyproject.get("dependency-groups", {}).get("dev", []))
    return [re.split(r"[<=>\\[\\s;]", dep, 1)[0] for dep in deps]


def _lock_packages(lock: dict[str, Any]) -> dict[str, dict[str, Any]]:
    packages: dict[str, dict[str, Any]] = {}
    for pkg in lock.get("package", []):
        name = _normalize_name(pkg.get("name", ""))
        if name:
            packages[name] = pkg
    return packages


def _lock_package_graph(
    lock: dict[str, Any],
    env: dict[str, str] | None = None,
    selected_extras: dict[str, set[str]] | None = None,
) -> tuple[dict[str, dict[str, Any]], dict[str, list[str]], list[dict[str, Any]]]:
    packages: dict[str, dict[str, Any]] = {}
    deps: dict[str, list[str]] = {}
    skipped: list[dict[str, Any]] = []
    env = env or _marker_environment()
    selected_extras = selected_extras or {}
    for pkg in lock.get("package", []):
        name = _normalize_name(pkg.get("name", ""))
        if not name:
            continue
        packages[name] = pkg
        dep_names: list[str] = []
        extras = selected_extras.get(name, set())
        for dep in pkg.get("dependencies", []):
            dep_name = _normalize_name(dep.get("name", ""))
            marker = dep.get("marker")
            extra = dep.get("extra")
            if extra and extra not in extras:
                skipped.append(
                    {
                        "name": dep.get("name"),
                        "from": pkg.get("name"),
                        "marker": marker,
                        "extra": extra,
                    }
                )
                continue
            if marker and not _marker_satisfied(marker, env, extras):
                skipped.append(
                    {
                        "name": dep.get("name"),
                        "from": pkg.get("name"),
                        "marker": marker,
                        "extra": extra,
                    }
                )
                continue
            if dep_name:
                dep_names.append(dep_name)
        deps[name] = dep_names
    return packages, deps, skipped


def _resolve_dependency_closure(
    roots: list[str],
    deps: dict[str, list[str]],
) -> tuple[list[str], list[str]]:
    seen: set[str] = set()
    missing: list[str] = []
    queue = list(roots)
    while queue:
        name = queue.pop(0)
        if name in seen:
            continue
        seen.add(name)
        if name not in deps:
            missing.append(name)
            continue
        for child in deps.get(name, []):
            if child not in seen:
                queue.append(child)
    return sorted(seen), sorted(set(missing))


def _pick_vendor_artifact(pkg: dict[str, Any]) -> tuple[str, dict[str, Any]] | None:
    for wheel in pkg.get("wheels", []):
        url = wheel.get("url", "")
        if "py3-none-any" in url:
            return "wheel", wheel
    sdist = pkg.get("sdist")
    if sdist:
        return "sdist", sdist
    wheels = pkg.get("wheels", [])
    if wheels:
        return "wheel", wheels[0]
    return None


def _download_artifact(url: str, expected_hash: str) -> bytes:
    if not url or not expected_hash:
        raise ValueError("missing url or hash")
    # TODO(tooling, owner:cli, milestone:TL2): add local cache for vendored artifacts.
    with urllib.request.urlopen(url) as response:
        data = response.read()
    digest = hashlib.sha256(data).hexdigest()
    expected = expected_hash.split(":", 1)[-1]
    if digest != expected:
        raise ValueError("hash mismatch")
    return data


def _classify_tier(
    name: str,
    pkg: dict[str, Any] | None,
    allow: dict[str, set[str]],
) -> tuple[str, str]:
    norm = _normalize_name(name)
    if norm in allow["tier_a"]:
        return "Tier A", _append_feature_notes("allowlisted", pkg)
    if norm in allow["tier_b"]:
        return "Tier B", _append_feature_notes("allowlisted", pkg)
    if norm in allow["tier_c"]:
        return "Tier C", _append_feature_notes("allowlisted", pkg)
    if norm in allow["native_wheels"]:
        return "Tier B", _append_feature_notes("allowlisted native wheels", pkg)

    molt_packages = {"molt_json", "molt_msgpack", "molt_cbor"}
    if norm in molt_packages:
        return "Tier B", _append_feature_notes("molt package", pkg)
    if pkg is None:
        return "Tier A", _append_feature_notes("unresolved (assumed pure python)", pkg)
    source = pkg.get("source", {})
    if source.get("git") or source.get("path"):
        return "Tier A", _append_feature_notes("local/git source", pkg)
    wheels = pkg.get("wheels", [])
    has_universal = any("py3-none-any" in wheel.get("url", "") for wheel in wheels)
    has_abi3 = any("abi3" in wheel.get("url", "") for wheel in wheels)
    if wheels and not has_universal and not has_abi3:
        return "Tier C", _append_feature_notes("platform wheels only", pkg)
    if has_abi3 and not has_universal:
        return "Tier B", _append_feature_notes("abi3 wheels", pkg)
    if wheels:
        return "Tier A", _append_feature_notes("universal wheels", pkg)
    if pkg.get("sdist"):
        return "Tier A", _append_feature_notes("sdist only", pkg)
    return "Tier A", _append_feature_notes("assumed pure python", pkg)


def _dep_allowlists(pyproject: dict[str, Any]) -> dict[str, set[str]]:
    tool_cfg = pyproject.get("tool", {}).get("molt", {}).get("deps", {})
    return {
        "tier_a": {_normalize_name(name) for name in tool_cfg.get("tier_a", [])},
        "tier_b": {_normalize_name(name) for name in tool_cfg.get("tier_b", [])},
        "tier_c": {_normalize_name(name) for name in tool_cfg.get("tier_c", [])},
        "native_wheels": {
            _normalize_name(name) for name in tool_cfg.get("native_wheels", [])
        },
    }


def _append_feature_notes(reason: str, pkg: dict[str, Any] | None) -> str:
    if not pkg:
        return reason
    metadata = pkg.get("metadata", {})
    requires = metadata.get("requires-dist", [])
    markers = any("marker" in dep for dep in requires)
    extras = any("extra" in dep for dep in requires)
    notes: list[str] = []
    if markers:
        notes.append("markers")
    if extras:
        notes.append("extras")
    if notes:
        return f"{reason}; {', '.join(notes)}"
    return reason


def _collect_py_files(target: Path) -> list[Path]:
    if target.is_file():
        return [target]
    return sorted(path for path in target.rglob("*.py") if path.is_file())


def _run_ty_check(path: Path) -> tuple[bool, str]:
    commands = [
        ["uv", "run", "ty", "check", str(path), "--output-format", "concise"],
        ["ty", "check", str(path), "--output-format", "concise"],
    ]
    for cmd in commands:
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, check=False)
        except FileNotFoundError:
            continue
        if result.returncode == 0:
            return True, result.stdout.strip()
        combined = (result.stdout + result.stderr).strip()
        return False, combined
    return False, "ty is not available; install it with `uv add ty`."


def _collect_type_facts_for_build(
    paths: list[Path], type_hint_policy: TypeHintPolicy, ty_target: Path
) -> tuple[Any | None, bool]:
    trust = "trusted" if type_hint_policy == "trust" else "guarded"
    ty_ok, _ = _run_ty_check(ty_target)
    facts = collect_type_facts_from_paths(paths, trust, infer=ty_ok)
    if ty_ok:
        facts.tool = "molt-check+ty+infer"
    return facts, ty_ok


def check(
    path: str,
    output: str,
    strict: bool,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    target = Path(path)
    if not target.exists():
        return _fail(f"Path not found: {target}", json_output, command="check")
    files = _collect_py_files(target)
    if not files:
        return _fail(
            f"No Python files found under: {target}",
            json_output,
            command="check",
        )
    trust = "trusted" if strict else "guarded"
    ty_ok, ty_output = _run_ty_check(target)
    warnings: list[str] = []
    if ty_ok:
        facts = collect_type_facts_from_paths(files, trust, infer=True)
        facts.tool = "molt-check+ty+infer"
        if verbose and not json_output:
            print("ty check passed; trusting inferred hints.")
    elif ty_output:
        warnings.append(ty_output)
        if not json_output:
            print(ty_output, file=sys.stderr)
        if strict:
            return _fail(
                "ty check failed; refusing strict type facts.",
                json_output,
                command="check",
            )
        facts = collect_type_facts_from_paths(files, trust, infer=False)
    else:
        facts = collect_type_facts_from_paths(files, trust, infer=False)
    output_path = Path(output)
    write_type_facts(output_path, facts)
    if json_output:
        payload = _json_payload(
            "check",
            "ok",
            data={"output": str(output_path), "strict": strict, "ty_ok": ty_ok},
            warnings=warnings,
        )
        _emit_json(payload, json_output)
    else:
        print(f"Wrote type facts to {output_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
