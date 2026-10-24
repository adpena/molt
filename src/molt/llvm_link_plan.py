"""One native LLVM dependency plan, selected before Cargo or SDK probes run.

The SDK owns operand order. A release capability may select real static
archives in place of nonportable shared dependencies; it never admits a final
binary. The binary auditor alone decides whether retained optional shared
operands were referenced. No policy package is imported by this module.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from typing import TYPE_CHECKING, Literal

from molt.process_environment import ProcessEnvironment

if TYPE_CHECKING:
    from molt.llvm_toolchain import LlvmPrefixVerification

LLVM_NATIVE_LINK_PLAN_ENV = "MOLT_LLVM_LINK_PLAN"
_LINK_NAME = re.compile(r"[A-Za-z0-9_+.-]+")


@dataclass(frozen=True)
class LlvmLinkLibrary:
    kind: Literal["static", "dylib", "framework"]
    name: str
    path: Path
    source: Literal["sdk", "system"]
    policy_admitted: bool | None


@dataclass(frozen=True)
class LlvmNativeLinkPlan:
    target_os: str
    target_arches: tuple[str, ...]
    host_target: str
    host_abi: Mapping[str, object]
    llvm_config: Path
    llvm_version: str
    driver: Path
    libraries: tuple[LlvmLinkLibrary, ...]
    policy: Mapping[str, object] | None

    @property
    def system_paths(self) -> tuple[Path, ...]:
        return tuple(lib.path for lib in self.libraries if lib.source == "system")

    def payload(self) -> dict[str, object]:
        return {
            "schema": "molt.llvm-native-link.v1",
            "target_os": self.target_os,
            "target_arches": list(self.target_arches),
            "host_target": self.host_target,
            "host_abi": dict(self.host_abi),
            "llvm_config": str(self.llvm_config),
            "llvm_version": self.llvm_version,
            "driver": str(self.driver),
            "libraries": [
                {
                    "kind": lib.kind,
                    "name": lib.name,
                    "path": str(lib.path),
                    "source": lib.source,
                    "policy_admitted": lib.policy_admitted,
                }
                for lib in self.libraries
            ],
            "policy": None
            if self.policy is None
            else {
                **self.policy,
                "shared_libraries": list(self.policy["shared_libraries"]),
            },
        }

    def environment_value(self) -> str:
        return json.dumps(self.payload(), sort_keys=True, separators=(",", ":"))


def _library_name(token: str) -> str:
    if token.startswith("-l"):
        name = token[2:]
    else:
        name = Path(token).name
        if name.lower().endswith(".lib"):
            name = name[:-4]
        elif name.startswith("lib"):
            name = name[3:]
            name = re.split(r"\.so(?:\.|$)|\.(?:a|dylib|tbd)$", name, maxsplit=1)[0]
        else:
            raise ValueError(f"unsupported LLVM library operand: {token!r}")
    # Versioned OpenBSD selectors and Darwin SDK stubs retain the SDK's family.
    name = re.split(r"\.so(?:\.|$)", name, maxsplit=1)[0]
    if name.startswith("lib") and name.endswith(".tbd"):
        name = name[3:-4]
    if _LINK_NAME.fullmatch(name) is None:
        raise ValueError(f"invalid LLVM library name: {token!r}")
    return name


def _resolve_file(names: tuple[str, ...], directories: tuple[Path, ...]) -> Path | None:
    # Match the selected driver's directory order, then dynamic/static preference
    # inside each directory. Never search the invoking project's working directory.
    for directory in directories:
        for name in names:
            path = directory / name
            if path.is_file():
                return path.absolute()
    return None


def _shared_permitted(name: str, names: frozenset[str]) -> bool:
    # llvm-config's -l spelling omits the ELF SONAME ABI suffix. The maintained
    # capability owns that list; this only maps its exact library family.
    return any(
        item == f"lib{name}.so" or item.startswith(f"lib{name}.so.") for item in names
    )


def _driver_library_directories(
    prefix: Path, driver: Path, environment: Mapping[str, str], target_os: str
) -> tuple[Path, ...]:
    from molt import process_guard

    if target_os == "windows":
        return (
            prefix / "lib",
            *(
                Path(item)
                for item in environment.get("LIB", "").split(os.pathsep)
                if item
            ),
        )
    completed = process_guard.run_completed_command(
        [str(driver), "--no-default-config", "--print-search-dirs"],
        env=ProcessEnvironment(environment),
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
        timeout=30,
        memory_guard_prefix="MOLT_LLVM",
    )
    answers = [
        line.removeprefix("libraries: =")
        for line in completed.stdout.splitlines()
        if line.startswith("libraries: =")
    ]
    if len(answers) != 1:
        raise ValueError("LLVM driver did not declare one library search path")
    directories = tuple(
        Path(item)
        for item in answers[0].split(os.pathsep)
        if item and Path(item).is_absolute()
    )
    if target_os == "macos":
        directories = (*directories, Path(environment["SDKROOT"]) / "usr/lib")
    if not directories:
        raise ValueError(
            "native LLVM dependency plan has no selected library search roots"
        )
    return directories


def _system_library(
    token: str,
    directories: tuple[Path, ...],
    target_os: str,
    allowed: frozenset[str] | None,
) -> LlvmLinkLibrary:
    name = _library_name(token)
    if Path(token).is_absolute():
        path = Path(token)
    else:
        names = (
            (f"{name}.lib",)
            if target_os == "windows"
            else (f"lib{name}.tbd", f"lib{name}.dylib", f"lib{name}.a")
            if target_os == "macos"
            else (f"lib{name}.so", f"lib{name}.a")
        )
        path = _resolve_file(names, directories)
    if path is None or not path.is_file():
        raise ValueError(f"LLVM system dependency has no selected file: {token}")
    kind: Literal["static", "dylib", "framework"] = (
        "static" if path.suffix == ".a" else "dylib"
    )
    permitted = (
        None
        if allowed is None
        else kind == "static" or _shared_permitted(name, allowed)
    )
    if permitted is False:
        archive = _resolve_file((f"lib{name}.a",), (path.parent, *directories))
        if archive is not None:
            path, kind, permitted = archive, "static", True
    return LlvmLinkLibrary(kind, name, path, "system", permitted)


def _abi_libraries(
    verification: LlvmPrefixVerification, environ: Mapping[str, str], target_os: str
) -> tuple[str, ...]:
    # clang++ adds this implicitly; the Rust native metadata must carry the same ABI.
    cpp = environ.get("CXXSTDLIB", environ.get("LLVM_SYS_LIBCPP"))
    if cpp is None and target_os != "windows":
        cpp = "c++" if target_os in {"macos", "freebsd", "openbsd"} else "stdc++"
    extras = [] if not cpp else [cpp]
    version = verification.version.split(".")
    binding = str(int(version[0]) * 10 + int(version[1]))
    if environ.get(f"LLVM_SYS_{binding}_FFI_WORKAROUND") is not None:
        extras.append("ffi")
    if target_os == "windows":
        from molt.llvm_toolchain import _run_llvm_config

        mode = _run_llvm_config(verification.llvm_config, "--build-mode")
        if mode not in {"Release", "Debug"}:
            raise ValueError(f"unsupported LLVM build mode: {mode!r}")
        if (
            mode == "Debug"
            or environ.get(f"LLVM_SYS_{binding}_USE_DEBUG_MSVCRT") is not None
        ):
            extras.append("msvcrtd")
    return tuple(extras)


def _rust_target_arches(cfg: str) -> tuple[str, ...]:
    term = r'target_arch\s*=\s*"([A-Za-z0-9_]+)"'
    simplified = re.sub(term, "A", cfg)
    if re.fullmatch(r"A|any\(A(?:,\s*A)+\)", simplified) is None:
        raise ValueError(f"unsupported native LLVM architecture contract: {cfg!r}")
    return tuple(sorted(set(re.findall(term, cfg))))


def select_native_llvm_link_plan(
    root: Path,
    verification: LlvmPrefixVerification,
    *,
    environ: Mapping[str, str],
    shared_capability: tuple[Mapping[str, object], frozenset[str]] | None = None,
) -> LlvmNativeLinkPlan:
    from molt.llvm_toolchain import llvm_host_architecture, llvm_sdk_probe_environment
    from molt.toolchain_identity import stable_executable_probe

    target_os = {"darwin": "macos", "win32": "windows"}.get(sys.platform, sys.platform)
    if target_os not in {"linux", "macos", "windows", "freebsd", "openbsd"}:
        raise ValueError(f"native LLVM linkage has no target plan for {sys.platform}")
    if shared_capability is not None and target_os != "linux":
        raise ValueError("ELF shared-library capability requires a Linux LLVM target")
    driver_facts = tuple(
        fact for fact in verification.tool_versions if fact.role == "clang++"
    )
    if len(driver_facts) != 1:
        raise ValueError("native LLVM link plan needs one admitted clang++ driver")
    driver = verification.prefix / driver_facts[0].path
    environment = llvm_sdk_probe_environment(environ)
    with stable_executable_probe(driver, label="LLVM dependency selection") as (
        _,
        identity,
    ):
        if (identity.sha256, identity.size) != (
            driver_facts[0].sha256,
            driver_facts[0].size,
        ):
            raise ValueError("LLVM dependency driver changed after SDK verification")
        try:
            directories = _driver_library_directories(
                verification.prefix, driver, environment, target_os
            )
        except (OSError, subprocess.SubprocessError) as exc:
            raise ValueError(f"LLVM dependency search query failed: {exc}") from exc
    policy = None if shared_capability is None else shared_capability[0]
    allowed = None if shared_capability is None else shared_capability[1]
    libraries: list[LlvmLinkLibrary] = []
    operands = iter(verification.link_closure)
    for operand in operands:
        if not operand.startswith("system:"):
            path = verification.prefix / operand
            libraries.append(
                LlvmLinkLibrary(
                    "static",
                    _library_name(str(path)),
                    path,
                    "sdk",
                    True if allowed is not None else None,
                )
            )
        elif operand == "system:-framework":
            token = next(operands, "")
            name = token.removeprefix("system:")
            if (
                target_os != "macos"
                or not token.startswith("system:")
                or _LINK_NAME.fullmatch(name) is None
            ):
                raise ValueError("LLVM framework selector has no valid target/name")
            base = (
                Path(environment["SDKROOT"])
                / "System/Library/Frameworks"
                / f"{name}.framework"
            )
            path = _resolve_file((f"{name}.tbd", name), (base, base / "Versions/A"))
            if path is None:
                raise ValueError(f"LLVM framework has no selected SDK file: {name}")
            libraries.append(LlvmLinkLibrary("framework", name, path, "system", None))
        else:
            libraries.append(
                _system_library(operand[7:], directories, target_os, allowed)
            )
    for name in _abi_libraries(verification, environ, target_os):
        if not any(lib.name == name for lib in libraries):
            libraries.append(
                _system_library("-l" + name, directories, target_os, allowed)
            )
    if len({lib.path for lib in libraries}) != len(libraries):
        raise ValueError("LLVM native link plan contains duplicate library operands")
    architecture = llvm_host_architecture(root, verification.host_target.split("-")[0])
    if architecture is None:
        raise ValueError(
            f"native LLVM linkage has no architecture plan for {verification.host_target}"
        )
    from molt.native_target_shape import native_artifact_shape

    shape = native_artifact_shape(
        verification.host_target.split("-")[0], target_triple=verification.host_target
    )
    return LlvmNativeLinkPlan(
        target_os,
        _rust_target_arches(architecture.rust_cfg),
        verification.host_target,
        {
            "architecture": shape.architecture,
            "pointer_bits": shape.pointer_bits,
            "header_bits": shape.header_bits,
            "byte_order": shape.byte_order,
        },
        verification.llvm_config,
        verification.version,
        driver,
        tuple(libraries),
        policy,
    )


def native_link_plan_record(
    plan: LlvmNativeLinkPlan, prefix: Path, resources
) -> dict[str, object]:
    """Location-neutral projection of the actual consumed files and linkage."""
    record = plan.payload()
    record["llvm_config"] = "${compiler/llvm-config}"
    record["driver"] = "${compiler/llvm-driver}"
    system_index = 0
    rows = []
    for lib in plan.libraries:
        if lib.source == "sdk":
            label = "compiler/llvm-sdk/" + lib.path.relative_to(prefix).as_posix()
        else:
            label = f"compiler/llvm-system/{system_index}"
            system_index += 1
        identity = resources.file_identity(lib.path)
        rows.append(
            {
                "kind": lib.kind,
                "name": lib.name,
                "path": "${" + label + "}",
                "source": lib.source,
                "policy_admitted": lib.policy_admitted,
                "size": identity.size,
                "sha256": identity.sha256,
            }
        )
    record["libraries"] = rows
    return record


def _validate_shared_policy(policy: object) -> frozenset[str]:
    keys = {
        "tool",
        "version",
        "platform",
        "arch",
        "policy_name",
        "policy_sha256",
        "shared_libraries",
    }
    if not isinstance(policy, dict) or policy.keys() != keys:
        raise ValueError("invalid native LLVM shared-library capability identity")
    names = policy["shared_libraries"]
    if (
        policy["tool"] != "auditwheel"
        or policy["platform"] != "linux"
        or not isinstance(policy["version"], str)
        or not policy["version"]
        or not isinstance(policy["arch"], str)
        or _LINK_NAME.fullmatch(policy["arch"]) is None
        or not isinstance(policy["policy_name"], str)
        or re.fullmatch(
            r"manylinux_\d+_\d+_" + re.escape(policy["arch"]), policy["policy_name"]
        )
        is None
        or not isinstance(policy["policy_sha256"], str)
        or re.fullmatch(r"[0-9a-f]{64}", policy["policy_sha256"]) is None
        or not isinstance(names, list)
        or not all(
            isinstance(name, str) and _LINK_NAME.fullmatch(name) for name in names
        )
        or names != sorted(set(names))
    ):
        raise ValueError("invalid native LLVM shared-library capability identity")
    return frozenset(names)


def validate_native_link_plan_record(
    value: object, roots: set[str]
) -> dict[str, object]:
    """Admit only the closed portable projection, never ambient search names."""
    keys = {
        "schema",
        "target_os",
        "target_arches",
        "host_target",
        "host_abi",
        "llvm_config",
        "llvm_version",
        "driver",
        "libraries",
        "policy",
    }
    if not isinstance(value, dict) or value.keys() != keys:
        raise ValueError("invalid native LLVM dependency plan")
    if (
        value["schema"] != "molt.llvm-native-link.v1"
        or value["target_os"] not in {"linux", "macos", "windows", "freebsd", "openbsd"}
        or not isinstance(value["target_arches"], list)
        or not value["target_arches"]
        or not all(
            isinstance(item, str) and re.fullmatch(r"[A-Za-z0-9_]+", item)
            for item in value["target_arches"]
        )
        or value["target_arches"] != sorted(set(value["target_arches"]))
        or not isinstance(value["llvm_version"], str)
        or re.fullmatch(r"\d+\.\d+\.\d+", value["llvm_version"]) is None
        or value["llvm_config"] != "${compiler/llvm-config}"
        or value["driver"] != "${compiler/llvm-driver}"
        or not {"compiler/llvm-config", "compiler/llvm-driver"}.issubset(roots)
    ):
        raise ValueError("invalid native LLVM dependency target or tool identity")
    from molt.native_target_shape import native_artifact_shape

    host_target = value["host_target"]
    if (
        not isinstance(host_target, str)
        or re.fullmatch(r"[A-Za-z0-9_+.-]+", host_target) is None
        or "-" not in host_target
    ):
        raise ValueError("invalid native LLVM SDK host target")
    shape = native_artifact_shape(host_target.split("-")[0], target_triple=host_target)
    if value["host_abi"] != {
        "architecture": shape.architecture,
        "pointer_bits": shape.pointer_bits,
        "header_bits": shape.header_bits,
        "byte_order": shape.byte_order,
    }:
        raise ValueError("native LLVM SDK host ABI differs from selected target shape")
    policy = value["policy"]
    if policy is not None and (
        value["target_os"] != "linux" or not isinstance(policy, dict)
    ):
        raise ValueError("invalid native LLVM shared-library capability")
    permitted_names = None if policy is None else _validate_shared_policy(policy)
    rows = value["libraries"]
    if not isinstance(rows, list) or not rows:
        raise ValueError("native LLVM dependency plan must be nonempty")
    seen = set()
    sdk = False
    system_index = 0
    for row in rows:
        if not isinstance(row, dict) or row.keys() != {
            "kind",
            "name",
            "path",
            "source",
            "policy_admitted",
            "size",
            "sha256",
        }:
            raise ValueError("invalid native LLVM dependency row")
        path = row["path"]
        if (
            row["kind"] not in {"static", "dylib", "framework"}
            or not isinstance(row["name"], str)
            or _LINK_NAME.fullmatch(row["name"]) is None
            or not isinstance(path, str)
            or not path.startswith("${")
            or not path.endswith("}")
            or path[2:-1] not in roots
            or path in seen
            or type(row["size"]) is not int
            or row["size"] <= 0
            or not isinstance(row["sha256"], str)
            or re.fullmatch(r"[0-9a-f]{64}", row["sha256"]) is None
            or (
                row["policy_admitted"] is not None
                and type(row["policy_admitted"]) is not bool
            )
            or (policy is None) != (row["policy_admitted"] is None)
        ):
            raise ValueError("native LLVM dependency lacks exact file custody")
        if permitted_names is not None and row["policy_admitted"] != (
            row["kind"] == "static" or _shared_permitted(row["name"], permitted_names)
        ):
            raise ValueError("native LLVM dependency misstates shared capability")
        if row["kind"] == "framework" and value["target_os"] != "macos":
            raise ValueError("native LLVM framework requires a Darwin target")
        seen.add(path)
        if row["source"] == "sdk":
            if (
                row["kind"] != "static"
                or not path.startswith("${compiler/llvm-sdk/lib/")
                or Path(path[:-1]).suffix.lower() not in {".a", ".lib"}
            ):
                raise ValueError(
                    "LLVM SDK dependency is not an admitted static archive"
                )
            sdk = True
        elif row["source"] == "system":
            if path != f"${{compiler/llvm-system/{system_index}}}":
                raise ValueError(
                    "native LLVM system dependency order or custody differs"
                )
            system_index += 1
        else:
            raise ValueError("invalid native LLVM dependency origin")
    if not sdk:
        raise ValueError("native LLVM dependency plan lacks static SDK archives")
    return value
