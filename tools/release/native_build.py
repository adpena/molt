"""One snapshot build and receipt authority for all native release executables."""

from __future__ import annotations

import argparse
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import asdict
import os
from pathlib import Path
import re
import shlex
import sys
import tempfile
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.cargo_execution_policy import (  # noqa: E402
    CARGO_WRAPPER_ENV_NAMES,
    admit_cargo_build,
)
from molt.cli.native_binary import validate_native_binary_architecture  # noqa: E402
from molt.cli.compiler_identity import LlvmCompilerInputs, admit_llvm_compiler_inputs  # noqa: E402
from molt.llvm_toolchain import (  # noqa: E402
    llvm_release,
    required_llvm_backend_pin,
    required_llvm_targets_for_host,
)
from molt.portable_paths import portable_relative_path  # noqa: E402
from molt.cli.runtime_build_python import build_python_scope  # noqa: E402
from molt.cli.runtime_identity_schema import _validated_build_python_identity  # noqa: E402
from molt.compiler_distribution import (  # noqa: E402
    PRODUCTION_COMPILER_FEATURES,
    PRODUCTION_COMPILER_PROFILE,
)
from molt.exact_json import canonical_json_sha256, read_exact, write_exact  # noqa: E402
from molt.temporary_artifacts import OwnedTemporaryDirectory  # noqa: E402
from molt.file_publication import (  # noqa: E402
    durable_publish_directory_exclusive,
    resolve_owned_path,
)
from molt.release_matrix import RUST_TARGET_BY_COORDINATE  # noqa: E402
from molt.platform_toolchain import (  # noqa: E402
    SDK_ENVIRONMENT_NAMES,
    activate_msvc_environment,
    select_darwin_toolchain,
    DarwinToolchain,
)
from molt.rust_toolchain import (  # noqa: E402
    cargo_configuration_paths,
    resolve_rustup_proxy,
    rust_channel,
    rustc_host,
)
from molt.toolchain_identity import (  # noqa: E402
    native_executable_content_identity,
    probe_executable,
    resolve_executable,
    snapshot_stable_regular_file,
    stable_regular_file_content_identity,
)
from tools.command_execution import CommandExecutor  # noqa: E402
from tools.git_identity import require_git_object_id  # noqa: E402

from tools.release.compiler_payload import source_environment, source_snapshot  # noqa: E402
from tools.release.git_source_snapshot import (  # noqa: E402
    GitSourceSnapshot,
    materialize_git_source_snapshot,
    read_git_source_file,
)
from tools.release.release_model import ROOT, target_by_id  # noqa: E402

SCHEMA = "molt.release-native-build.v3"
RECEIPT_NAME = "native-build.json"
_COMMANDS = CommandExecutor.for_file(__file__)
_COMPONENTS = {
    "compiler": (
        "molt-backend",
        "molt-backend",
        PRODUCTION_COMPILER_PROFILE,
        PRODUCTION_COMPILER_FEATURES,
    ),
    "launcher": ("molt-launcher", "molt", PRODUCTION_COMPILER_PROFILE, ()),
    "worker": ("molt-worker", "molt-worker", "release-output", ()),
}
_TOOL_ROLES = frozenset(
    {
        "cargo",
        "rustc",
        "cc",
        "cxx",
        "ar",
        "ranlib",
        "linker",
        "linker_backend",
        "python",
        "cmake",
        "ninja",
    }
)
# Build policy is authored below. Ambient flags, wrappers, per-target overrides,
# Python injection and arbitrary Cargo/MOLT variables never enter this map.
_PLATFORM_ENV = (
    frozenset(
        {
            "COMSPEC",
            "HOME",
            "HOMEDRIVE",
            "HOMEPATH",
            "PATH",
            "PATHEXT",
            "SYSTEMDRIVE",
            "SYSTEMROOT",
            "TEMP",
            "TMP",
            "TMPDIR",
            "USERPROFILE",
            "WINDIR",
            "RUSTUP_HOME",
        }
    )
    | SDK_ENVIRONMENT_NAMES
)


def source_record(snapshot: GitSourceSnapshot) -> dict[str, Any]:
    return {
        "object_format": snapshot.object_format,
        "commit": snapshot.source_sha,
        "tree": snapshot.tree_sha,
        "files_sha256": canonical_json_sha256(
            [entry.as_record() for entry in snapshot.files]
        ),
    }


LLVM_POLICY_PATHS = (
    "config/llvm_toolchain_arches.toml",
    "config/llvm_toolchain_releases.toml",
    "runtime/molt-backend-native/Cargo.toml",
    "runtime/molt-backend/Cargo.toml",
)


def llvm_policy_sha256(files: Iterable[Mapping[str, Any]]) -> str:
    selected = {
        item["path"]: item for item in files if item["path"] in LLVM_POLICY_PATHS
    }
    if any(path not in selected for path in LLVM_POLICY_PATHS):
        raise ValueError("native build source omits LLVM policy authorities")
    return canonical_json_sha256(
        [
            {
                "path": path,
                "size": selected[path]["size"],
                "sha256": selected[path]["sha256"],
            }
            for path in LLVM_POLICY_PATHS
        ]
    )


def llvm_input_record(
    inputs: LlvmCompilerInputs, snapshot: GitSourceSnapshot
) -> dict[str, Any]:
    verification = inputs.verification
    if verification.release is None:
        raise ValueError("release compiler requires a manifest-pinned LLVM release")
    config = inputs.resources.file_identity(verification.llvm_config)
    closure = [
        f"system:${{compiler/llvm-system/{index}}}"
        if token.startswith("system:") and Path(token[7:]).is_absolute()
        else token
        for index, token in enumerate(verification.link_closure)
    ]
    return validate_llvm_inputs(
        {
            "linkage": "force-static",
            "version": verification.version,
            "upstream_release": asdict(verification.release),
            "policy_sha256": llvm_policy_sha256(
                item.as_record() for item in snapshot.files
            ),
            "targets": list(verification.targets),
            "link_closure": closure,
            "llvm_config": {"sha256": config.sha256, "size": config.size},
            "resources": inputs.resources.content_identity(),
        }
    )


def validate_llvm_inputs(value: object) -> dict[str, Any]:
    record = _object(
        value,
        {
            "linkage",
            "version",
            "upstream_release",
            "policy_sha256",
            "targets",
            "link_closure",
            "llvm_config",
            "resources",
        },
        "LLVM compiler inputs",
    )
    pin = required_llvm_backend_pin(ROOT)
    release = None if pin is None else llvm_release(pin.default_release, ROOT)
    if (
        release is None
        or record["linkage"] != "force-static"
        or record["version"] != release.version
        or record["upstream_release"] != asdict(release)
        or not _digest(record["policy_sha256"])
    ):
        raise ValueError("native LLVM input policy differs from pinned static release")
    targets = record["targets"]
    if (
        not isinstance(targets, list)
        or not targets
        or not all(
            isinstance(item, str) and re.fullmatch(r"[A-Za-z0-9]+", item)
            for item in targets
        )
        or targets != sorted(set(targets))
    ):
        raise ValueError("invalid native LLVM target inventory")
    config = _object(record["llvm_config"], {"sha256", "size"}, "llvm-config identity")
    if (
        not _digest(config["sha256"])
        or type(config["size"]) is not int
        or config["size"] <= 0
    ):
        raise ValueError("invalid native llvm-config identity")
    resources = _object(
        record["resources"],
        {"digest", "file_count", "total_size", "roots", "missing"},
        "LLVM resources",
    )
    roots = resources["roots"]
    if (
        not _digest(resources["digest"])
        or type(resources["file_count"]) is not int
        or resources["file_count"] <= 1
        or type(resources["total_size"]) is not int
        or resources["total_size"] < config["size"]
        or resources["missing"] != []
        or not isinstance(roots, list)
        or not all(
            isinstance(item, str)
            and re.fullmatch(r"compiler/llvm-(?:config|sdk/[^\\:]+|system/\d+)", item)
            for item in roots
        )
        or roots != sorted(set(roots))
        or resources["file_count"] != len(roots)
        or "compiler/llvm-config" not in roots
    ):
        raise ValueError("invalid native LLVM resource custody")
    for root in roots:
        portable_relative_path(root)
    closure = record["link_closure"]
    if (
        not isinstance(closure, list)
        or not closure
        or not all(isinstance(item, str) for item in closure)
    ):
        raise ValueError("invalid native LLVM static link closure")
    archives = []
    for index, token in enumerate(closure):
        if token.startswith("system:"):
            operand = token[7:]
            if operand == f"${{compiler/llvm-system/{index}}}":
                if f"compiler/llvm-system/{index}" not in roots:
                    raise ValueError("LLVM external system input lacks byte custody")
            elif (
                re.fullmatch(r"(?:-l[A-Za-z0-9_.+-]+|[A-Za-z0-9_.+-]+\.lib)", operand)
                is None
            ):
                raise ValueError("LLVM system selector is not location-neutral")
        else:
            if (
                not token.startswith("lib/")
                or ".." in Path(token).parts
                or Path(token).suffix.lower() not in {".a", ".lib"}
                or "compiler/llvm-sdk/" + token not in roots
            ):
                raise ValueError("LLVM archive lacks admitted static SDK bytes")
            archives.append(token)
    if not archives or len(archives) != len(set(archives)):
        raise ValueError("LLVM static archive inventory must be nonempty and unique")
    return record


def snapshot_rust_channel(repo_root: Path, snapshot: GitSourceSnapshot) -> str:
    env = source_environment()
    data = read_git_source_file(
        snapshot,
        "rust-toolchain.toml",
        repo_root=repo_root,
        git=resolve_executable("git", environment=env, label="release source Git"),
        environment=env,
        max_bytes=64 * 1024,
    )
    return rust_channel(data)


def tool_roles(platform: str, arch: str) -> set[str]:
    return set(_TOOL_ROLES) | (
        {"nasm"} if (platform, arch) == ("windows", "x86_64") else set()
    )


def require_config_free_build_root(work: Path, env: Mapping[str, str]) -> None:
    configs = cargo_configuration_paths(work, env)
    if configs:
        raise ValueError(
            "native release build rejects ambient Cargo configuration: "
            + ", ".join(str(path) for path in configs)
            + "; select --build-root outside these configuration ancestors"
        )


def select_build_root(requested: Path | None, inherited: Mapping[str, str]) -> Path:
    root = resolve_owned_path(
        requested or Path(inherited.get("RUNNER_TEMP") or tempfile.gettempdir())
    )
    root.mkdir(parents=True, exist_ok=True)
    require_config_free_build_root(
        root, {"CARGO_HOME": str(root / ".release-cargo-home-probe")}
    )
    return root


def component_plan(platform: str, arch: str) -> dict[str, Any]:
    """The exact command family, shared by construction and receipt admission."""
    triple = RUST_TARGET_BY_COORDINATE[(platform, arch)]
    suffix = ".exe" if platform == "windows" else ""
    return {
        role: {
            "package": package,
            "binary": binary,
            "profile": profile,
            "default_features": role != "compiler",
            "features": list(features),
            "target": triple,
            "path": f"bin/{binary}{suffix}",
        }
        for role, (package, binary, profile, features) in _COMPONENTS.items()
    }


def _object(value: object, fields: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != fields:
        raise ValueError(f"invalid native build {label}")
    return value


def _digest(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def _validate_rust_tools(tools: Mapping[str, Any], channel: str, triple: str) -> None:
    if (
        not tools["cargo"]["version"].startswith(f"cargo {channel} ")
        or not tools["rustc"]["version"].startswith(f"rustc {channel} ")
        or rustc_host(tools["rustc"]["version"]) != triple
    ):
        raise ValueError(
            "native build Rust identity differs from source channel or target host"
        )


def validate_receipt(payload: object) -> dict[str, Any]:
    receipt = _object(
        payload,
        {
            "schema",
            "source",
            "source_date_epoch",
            "target",
            "policy",
            "tools",
            "build_python",
            "llvm",
            "artifacts",
        },
        "receipt fields",
    )
    if (
        receipt["schema"] != SCHEMA
        or type(receipt["source_date_epoch"]) is not int
        or receipt["source_date_epoch"] <= 0
    ):
        raise ValueError("invalid native build schema or epoch")
    source = _object(
        receipt["source"], {"object_format", "commit", "tree", "files_sha256"}, "source"
    )
    length = {"sha1": 40, "sha256": 64}.get(
        source["object_format"] if isinstance(source["object_format"], str) else ""
    )
    for key in ("commit", "tree"):
        require_git_object_id(source[key], label=f"native build {key}")
        if len(source[key]) != length:
            raise ValueError("native build source object format mismatch")
    if not _digest(source["files_sha256"]):
        raise ValueError("invalid native build source inventory digest")
    target = _object(receipt["target"], {"platform", "arch", "rust_target"}, "target")
    if not all(isinstance(value, str) for value in target.values()):
        raise ValueError("invalid native build target values")
    triple = RUST_TARGET_BY_COORDINATE.get((target["platform"], target["arch"]))
    if triple is None or target["rust_target"] != triple:
        raise ValueError("native build target differs from release policy")
    policy = _object(
        receipt["policy"],
        {"revision", "rust_channel", "platform_versions", "components"},
        "policy",
    )
    if (
        policy["revision"] != 2
        or type(policy["revision"]) is not int
        or not isinstance(policy["rust_channel"], str)
        or not re.fullmatch(r"\d+\.\d+\.\d+", policy["rust_channel"])
    ):
        raise ValueError("invalid native build policy revision or Rust channel")
    if policy["components"] != component_plan(target["platform"], target["arch"]):
        raise ValueError("native build profile/features differ from release policy")
    if any(
        type(plan["default_features"]) is not bool
        for plan in policy["components"].values()
    ):
        raise ValueError("native build feature selection must be boolean")
    versions = receipt["policy"]["platform_versions"]
    if target["platform"] == "windows":
        versions = _object(
            versions, {"msvc", "windows_sdk", "ucrt"}, "Windows SDK versions"
        )
    elif target["platform"] == "macos":
        versions = _object(
            versions,
            {"sdk", "deployment_target", "sdk_settings_sha256"},
            "Darwin SDK identity",
        )
        if not _digest(versions["sdk_settings_sha256"]):
            raise ValueError("invalid native build Darwin SDK settings identity")
        versions = {
            key: value
            for key, value in versions.items()
            if key != "sdk_settings_sha256"
        }
    else:
        versions = _object(versions, set(), "Linux platform versions")
    if any(
        not isinstance(value, str) or re.fullmatch(r"\d+(?:\.\d+)+", value) is None
        for value in versions.values()
    ):
        raise ValueError("invalid native build platform version")
    tools = _object(
        receipt["tools"], tool_roles(target["platform"], target["arch"]), "tool roles"
    )
    for role, tool in tools.items():
        _object(
            tool,
            {"entrypoint", "content_filename", "sha256", "size", "version"},
            f"{role} identity",
        )
        if (
            not _digest(tool["sha256"])
            or type(tool["size"]) is not int
            or tool["size"] <= 0
        ):
            raise ValueError(f"invalid native build {role} content identity")
        for key in ("entrypoint", "content_filename"):
            if (
                not isinstance(tool[key], str)
                or not tool[key]
                or any(c in tool[key] for c in "/\\:\x00\r\n")
                or tool[key] in {".", ".."}
            ):
                raise ValueError(
                    f"native build {role} identity is not location-neutral"
                )
        if not isinstance(tool["version"], str):
            raise ValueError(f"invalid native build {role} version")
        if role not in {"cargo", "rustc"} and tool["version"]:
            raise ValueError("native non-Rust tool identity must use content bytes")
    channel = policy["rust_channel"]
    _validate_rust_tools(tools, channel, triple)
    python = _validated_build_python_identity(receipt["build_python"])
    if python["selected_executable"] != {
        key: value for key, value in tools["python"].items() if key != "version"
    }:
        raise ValueError("native build Python closure differs from selected executable")
    llvm = validate_llvm_inputs(receipt["llvm"])
    if not set(required_llvm_targets_for_host(ROOT, target["arch"])).issubset(
        llvm["targets"]
    ):
        raise ValueError("native LLVM SDK omits a required host code generator")
    artifacts = _object(receipt["artifacts"], set(_COMPONENTS), "artifacts")
    for role, artifact in artifacts.items():
        _object(artifact, {"path", "sha256", "size"}, f"{role} artifact")
        if (
            artifact["path"] != policy["components"][role]["path"]
            or not _digest(artifact["sha256"])
            or type(artifact["size"]) is not int
            or artifact["size"] <= 0
        ):
            raise ValueError(f"invalid native build {role} artifact")
    return receipt


def read_native_build(
    root: Path,
    *,
    snapshot: GitSourceSnapshot,
    expected_rust_channel: str,
    source_date_epoch: int,
    platform: str,
    arch: str,
) -> dict[str, Any]:
    receipt = validate_receipt(
        read_exact(
            root / RECEIPT_NAME, max_bytes=4 * 1024 * 1024, label="native build receipt"
        )
    )
    if (
        receipt["source"] != source_record(snapshot)
        or receipt["source_date_epoch"] != source_date_epoch
        or receipt["target"]
        != {
            "platform": platform,
            "arch": arch,
            "rust_target": RUST_TARGET_BY_COORDINATE[(platform, arch)],
        }
    ):
        raise ValueError(
            "native build receipt differs from candidate source, epoch or target"
        )
    if receipt["policy"]["rust_channel"] != expected_rust_channel:
        raise ValueError("native build Rust channel differs from source snapshot")
    if receipt["llvm"]["policy_sha256"] != llvm_policy_sha256(
        item.as_record() for item in snapshot.files
    ):
        raise ValueError("native LLVM policy differs from source snapshot")
    for role, record in receipt["artifacts"].items():
        binary = root / record["path"]
        validate_native_binary_architecture(binary, receipt["target"]["rust_target"])
        identity = stable_regular_file_content_identity(binary, label=f"release {role}")
        if (identity["sha256"], identity["size"]) != (record["sha256"], record["size"]):
            raise ValueError(f"native build {role} binary differs from receipt")
    return receipt


def build_environment(
    source: Path, work: Path, inherited: Mapping[str, str], *, epoch: int
) -> dict[str, str]:
    env = {
        key.upper(): value
        for key, value in inherited.items()
        if key.upper() in _PLATFORM_ENV
    }
    channel = rust_channel((source / "rust-toolchain.toml").read_bytes())
    env.update({name: "" for name in CARGO_WRAPPER_ENV_NAMES})
    env.update(
        {
            "CARGO_HOME": str(work / "cargo-home"),
            "CARGO_TARGET_DIR": str(work / "target"),
            "CARGO_INCREMENTAL": "0",
            "RUSTUP_TOOLCHAIN": channel,
            "SOURCE_DATE_EPOCH": str(epoch),
            "PYTHONNOUSERSITE": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
            "LC_ALL": "C",
            "TZ": "UTC",
        }
    )
    # Cargo searches the build cwd, not --manifest-path's ancestors. The private
    # cwd and fresh Cargo home prevent both config injection and artifact reuse.
    require_config_free_build_root(work, env)
    return env


def _tool_paths(
    source: Path,
    env: dict[str, str],
    *,
    platform: str,
    arch: str,
    darwin: DarwinToolchain | None = None,
) -> dict[str, Path]:
    names = {
        "cargo": "cargo",
        "rustc": "rustc",
        "cc": "clang",
        "cxx": "clang++",
        "ar": "ar",
        "ranlib": "ranlib",
        "linker": "clang",
        "cmake": "cmake",
        "ninja": "ninja",
    }
    if platform == "windows":
        names.update(
            cc="cl.exe", cxx="cl.exe", ar="lib.exe", ranlib="lib.exe", linker="link.exe"
        )
    if platform == "windows" and arch == "x86_64":
        names["nasm"] = "nasm"
    if darwin is not None:
        names.update(
            {
                role: str(darwin.tools[name])
                for role, name in (
                    ("cc", "clang"),
                    ("cxx", "clang++"),
                    ("ar", "ar"),
                    ("ranlib", "ranlib"),
                    ("linker", "clang"),
                )
            }
        )
    paths = {
        role: resolve_executable(
            name,
            environment=env,
            label=f"release {role} (required native dependency tool)",
        )
        for role, name in names.items()
    }
    for role in ("cargo", "rustc"):
        paths[role] = resolve_rustup_proxy(paths[role], role=role, root=source, env=env)
    paths["python"] = Path(sys.executable)
    paths["linker_backend"] = paths["linker"]
    if darwin is not None:
        paths["linker_backend"] = darwin.tools["ld"]
    elif platform != "windows":
        selected = _COMMANDS.check_output(
            [str(paths["linker"]), "-print-prog-name=ld"],
            cwd=source,
            env=env,
            text=True,
            timeout=30,
            encoding="utf-8",
        ).strip()
        if not selected or "\n" in selected or "\r" in selected:
            raise ValueError("native linker must select one backend path")
        paths["linker_backend"] = resolve_executable(
            selected, environment=env, label="native linker backend"
        )
    return paths


def _tool_identities(
    paths: Mapping[str, Path], env: Mapping[str, str]
) -> dict[str, Any]:
    records = {}
    for role, path in paths.items():
        if role in {"cargo", "rustc"}:
            record = probe_executable(
                path,
                version_arguments=(("--version", "--verbose"),),
                environment=env,
                label=f"release {role}",
            ).as_record()
        else:
            record = {
                **native_executable_content_identity(path, label=f"release {role}"),
                "version": "",
            }
        records[role] = record
    return records


def _select_build_tools(
    env: dict[str, str],
    paths: Mapping[str, Path],
    *,
    work: Path,
    triple: str,
    platform: str,
) -> None:
    # Pin drivers and the linker backend, including cc-rs target spellings.
    directories = list(dict.fromkeys(str(path.parent) for path in paths.values()))
    if platform == "windows":
        directories.append(str(Path(env["SYSTEMROOT"]) / "System32"))
    else:
        directories.extend(("/usr/bin", "/bin"))
    env["PATH"] = os.pathsep.join(dict.fromkeys(directories))
    for name, role in (
        ("CARGO", "cargo"),
        ("RUSTC", "rustc"),
        ("CC", "cc"),
        ("CXX", "cxx"),
        ("AR", "ar"),
        ("RANLIB", "ranlib"),
        ("PYTHON", "python"),
        ("MOLT_BUILD_PYTHON", "python"),
    ):
        env[name] = str(paths[role])
        if name in {"CC", "CXX", "AR", "RANLIB"}:
            for suffix in (triple, triple.replace("-", "_")):
                env[f"{name}_{suffix}"] = str(paths[role])
    env[f"CARGO_TARGET_{triple.replace('-', '_').upper()}_LINKER"] = str(
        paths["linker"]
    )
    rustflags = [f"--remap-path-prefix={work}=/molt", "-C", f"linker={paths['linker']}"]
    if platform == "windows":
        rustflags.extend(("-C", "link-arg=/Brepro", "-C", "link-arg=/DEBUG:NONE"))
        cflags = ["/Brepro", f"/pathmap:{work}=/molt"]
    else:
        rustflags.extend(("-C", f"link-arg=--ld-path={paths['linker_backend']}"))
        cflags = [f"-ffile-prefix-map={work}=/molt", f"-fdebug-prefix-map={work}=/molt"]
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(rustflags)
    env["CFLAGS"] = env["CXXFLAGS"] = shlex.join(cflags)
    env["CMAKE"] = str(paths["cmake"])
    env["CMAKE_GENERATOR"] = "Ninja"
    if "nasm" in paths:
        env["NASM"] = str(paths["nasm"])
    env["CC_SHELL_ESCAPED_FLAGS"] = "1"
    env["ZERO_AR_DATE"] = "1"
    if platform == "windows":
        env["ARFLAGS"] = "/Brepro"


def cargo_command(
    plan: Mapping[str, Any], *, source: Path, work: Path, cargo: Path
) -> list[str]:
    argv = [
        str(cargo),
        "build",
        "--locked",
        "--manifest-path",
        str(source / "Cargo.toml"),
        "--profile",
        plan["profile"],
        "--package",
        plan["package"],
        "--bin",
        plan["binary"],
        "--target",
        plan["target"],
        "--target-dir",
        str(work / "target"),
    ]
    if not plan["default_features"]:
        argv.append("--no-default-features")
    if plan["features"]:
        argv.extend(("--features", ",".join(plan["features"])))
    return argv


def produce_native_build(
    repo_root: Path,
    output: Path,
    *,
    source_sha: str,
    source_date_epoch: int,
    platform: str,
    arch: str,
    build_root: Path | None = None,
) -> dict[str, Any]:
    from molt.verified_subset import current_host_coordinate

    if current_host_coordinate() != (platform, arch):
        raise ValueError("native release binaries must be built on their target host")
    if type(source_date_epoch) is not int or source_date_epoch <= 0:
        raise ValueError("native release epoch must be a positive integer")
    output = resolve_owned_path(output)
    if output.exists():
        raise FileExistsError(f"native build destination already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    snapshot = source_snapshot(repo_root, source_sha)
    scratch = select_build_root(build_root, os.environ)
    with (
        OwnedTemporaryDirectory(
            prefix="molt-release-native-", dir=scratch
        ) as temporary,
        OwnedTemporaryDirectory(
            prefix=".native-publish-", dir=output.parent
        ) as publication,
    ):
        work = Path(temporary)
        stage = Path(publication) / "publish"
        (stage / "bin").mkdir(parents=True)
        git_env = source_environment()
        source = materialize_git_source_snapshot(
            snapshot,
            work / "source",
            repo_root=repo_root,
            git=resolve_executable(
                "git", environment=git_env, label="release source Git"
            ),
            environment=git_env,
        )
        # Platform SDK setup owns include/library search roots. An ambient LIB,
        # INCLUDE or SDKROOT must not survive as an unrecorded build override.
        inherited = {
            key: value
            for key, value in os.environ.items()
            if key.upper() not in SDK_ENVIRONMENT_NAMES
        }
        darwin = None
        versions = {}
        if platform == "windows":
            inherited = activate_msvc_environment(inherited, repo_root=source)
        elif platform == "macos":
            darwin = select_darwin_toolchain(inherited)
            inherited.update(darwin.environment())
            versions = darwin.versions()
        llvm_inputs = admit_llvm_compiler_inputs(source, inherited)
        env = build_environment(source, work, inherited, epoch=source_date_epoch)
        if platform == "windows":
            versions = {
                key: env.get(name, "").rstrip("/\\")
                for key, name in (
                    ("msvc", "VCTOOLSVERSION"),
                    ("windows_sdk", "WINDOWSSDKVERSION"),
                    ("ucrt", "UCRTVERSION"),
                )
            }
        paths = _tool_paths(source, env, platform=platform, arch=arch, darwin=darwin)
        triple = RUST_TARGET_BY_COORDINATE[(platform, arch)]
        _select_build_tools(env, paths, work=work, triple=triple, platform=platform)
        compiler_env = llvm_inputs.environment(env)
        with build_python_scope(None) as build_python:
            identities = _tool_identities(paths, env)
            _validate_rust_tools(identities, env["RUSTUP_TOOLCHAIN"], triple)
            plans = component_plan(platform, arch)
            receipt = {
                "schema": SCHEMA,
                "source": source_record(snapshot),
                "source_date_epoch": source_date_epoch,
                "target": {"platform": platform, "arch": arch, "rust_target": triple},
                "policy": {
                    "revision": 2,
                    "rust_channel": env["RUSTUP_TOOLCHAIN"],
                    "platform_versions": versions,
                    "components": plans,
                },
                "tools": identities,
                "build_python": build_python.capture(compiler_env),
                "llvm": llvm_input_record(llvm_inputs, snapshot),
                "artifacts": {},
            }
            for role, plan in plans.items():
                if role == "compiler":
                    llvm_inputs.verify()
                role_env = compiler_env if role == "compiler" else env
                command = cargo_command(
                    plan, source=source, work=work, cargo=paths["cargo"]
                )
                admit_cargo_build(command, cwd=work, env=role_env)
                _COMMANDS.run(command, cwd=work, env=role_env, check=True)
                binary = (
                    work / "target" / triple / plan["profile"] / Path(plan["path"]).name
                )
                destination = stage / plan["path"]
                captured = snapshot_stable_regular_file(
                    binary, destination, label=f"release {role} output"
                )
                validate_native_binary_architecture(destination, triple)
                receipt["artifacts"][role] = {
                    "path": plan["path"],
                    "sha256": captured.snapshot.sha256,
                    "size": captured.snapshot.size,
                }
                destination.chmod(0o755)
            llvm_inputs.verify()
            snapshot.verify(source)
            if darwin is not None and darwin != select_darwin_toolchain(
                env, developer_dir=darwin.developer_dir
            ):
                raise ValueError(
                    "Darwin SDK or tool selection changed during native build"
                )
            require_config_free_build_root(work, env)
            if identities != _tool_identities(paths, env):
                raise ValueError("native build tool identity changed during build")
            if receipt["build_python"] != build_python.capture(compiler_env):
                raise ValueError("native build Python runtime changed during build")
            write_exact(stage / RECEIPT_NAME, validate_receipt(receipt))
            read_native_build(
                stage,
                snapshot=snapshot,
                expected_rust_channel=env["RUSTUP_TOOLCHAIN"],
                source_date_epoch=source_date_epoch,
                platform=platform,
                arch=arch,
            )
        durable_publish_directory_exclusive(stage, output)
    return receipt


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--build-root",
        type=Path,
        help="config-free parent of private build directories; defaults to RUNNER_TEMP or system temp",
    )
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--source-date-epoch", type=int, required=True)
    parser.add_argument("--target", required=True)
    args = parser.parse_args(argv)
    target = target_by_id(args.target)
    produce_native_build(
        ROOT,
        args.output,
        source_sha=args.source_sha,
        source_date_epoch=args.source_date_epoch,
        platform=target.platform,
        arch=target.arch,
        build_root=args.build_root,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
