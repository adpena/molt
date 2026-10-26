"""Observed installed-package custody for release-selected static system archives.

dpkg declares ownership; it does not authenticate the archive as upstream bytes.
The admitted link plan separately binds the exact archive SHA-256 and size.
Full notices travel with the receipt, so relocation never rereads host notices.
Debian policy owns copyright/common-license locations:
https://www.debian.org/doc/debian-policy/ch-docs.html#copyright-information
"""

from __future__ import annotations

import base64
import binascii
import hashlib
import os
from pathlib import Path
import re
import subprocess
from typing import TYPE_CHECKING, Any, Mapping

from molt.portable_paths import portable_relative_path
from molt.process_environment import ProcessEnvironment
from molt.toolchain_identity import capture_stable_regular_file, resolve_executable
from tools.command_execution import CommandExecutor

if TYPE_CHECKING:
    from molt.cli.cargo_resource_custody import CargoResourceCustody
    from molt.llvm_link_plan import LlvmNativeLinkPlan

_COMMANDS = CommandExecutor.for_file(__file__)
_ROOT = Path(__file__).resolve().parents[2]
_COMMON = re.compile(rb"/usr/share/common-licenses/([A-Za-z0-9][A-Za-z0-9+_.-]*)")


def _common_notices(data: bytes) -> set[str]:
    return {
        "usr/share/common-licenses/" + name.decode("ascii")
        for name in _COMMON.findall(data)
    }


def notice_bytes(notice: Mapping[str, Any]) -> bytes:
    """Decode and check a receipt notice without consulting a host filesystem."""
    if set(notice) != {"path", "sha256", "size", "content_base64"}:
        raise ValueError("invalid static dependency notice fields")
    portable_relative_path(notice["path"])
    try:
        data = base64.b64decode(notice["content_base64"], validate=True)
    except (ValueError, TypeError, binascii.Error) as exc:
        raise ValueError("invalid static dependency notice bytes") from exc
    if (
        type(notice["size"]) is not int
        or notice["size"] <= 0
        or len(data) != notice["size"]
        or hashlib.sha256(data).hexdigest() != notice["sha256"]
        or base64.b64encode(data).decode("ascii") != notice["content_base64"]
    ):
        raise ValueError("static dependency notice content identity differs")
    return data


def validate_system_static_dependencies(
    value: object, link_plan_record: Mapping[str, Any]
) -> list[dict[str, Any]]:
    """Bind closed package/notice records to every selected system-static row."""
    if not isinstance(value, list):
        raise ValueError("static system dependencies must be a list")
    expected = {
        row["path"]: {key: row[key] for key in ("path", "name", "sha256", "size")}
        for row in link_plan_record["libraries"]
        if row["source"] == "system" and row["kind"] == "static"
    }
    observed = {}
    packages = []
    for item in value:
        if not isinstance(item, dict) or set(item) != {
            "package",
            "archives",
            "notices",
        }:
            raise ValueError("invalid static dependency package record")
        package = item["package"]
        if not isinstance(package, dict) or set(package) != {
            "manager",
            "name",
            "version",
            "architecture",
        }:
            raise ValueError("invalid static dependency package identity")
        if (
            package["manager"] != "dpkg"
            or not isinstance(package["name"], str)
            or re.fullmatch(r"[a-z0-9][a-z0-9+.-]*", package["name"]) is None
            or not isinstance(package["architecture"], str)
            or re.fullmatch(r"[a-z0-9][a-z0-9-]*", package["architecture"]) is None
            or not isinstance(package["version"], str)
            or re.fullmatch(r"[0-9][A-Za-z0-9.+:~\-]*", package["version"]) is None
        ):
            raise ValueError("invalid installed dpkg package identity")
        packages.append((package["name"], package["architecture"], package["version"]))
        archives = item["archives"]
        if not isinstance(archives, list) or not archives:
            raise ValueError("static package has no selected archives")
        paths = []
        for archive in archives:
            if not isinstance(archive, dict) or set(archive) != {
                "path",
                "name",
                "sha256",
                "size",
            }:
                raise ValueError("invalid static dependency archive identity")
            path = archive["path"]
            if (
                not isinstance(path, str)
                or path in observed
                or archive != expected.get(path)
                or type(archive["size"]) is not int
            ):
                raise ValueError(
                    "static dependency archive differs from selected link plan"
                )
            observed[path] = archive
            paths.append(path)
        if paths != sorted(paths):
            raise ValueError("static dependency archives are not canonical")
        notices = item["notices"]
        if not isinstance(notices, list) or not all(
            isinstance(row, dict) for row in notices
        ):
            raise ValueError("invalid static dependency notices")
        copyright_path = f"usr/share/doc/{package['name']}/copyright"
        contents = {}
        notice_paths = []
        for notice in notices:
            data = notice_bytes(notice)
            notice_paths.append(notice["path"])
            contents[notice["path"]] = data
        required = set()
        pending = {copyright_path}
        while pending:
            path = pending.pop()
            if path not in contents:
                raise ValueError(
                    "static dependency copyright/common-license closure differs"
                )
            required.add(path)
            pending.update(_common_notices(contents[path]) - required)
        if notice_paths != sorted(required):
            raise ValueError(
                "static dependency copyright/common-license closure differs"
            )
    if packages != sorted(set(packages)) or len({p[:2] for p in packages}) != len(
        packages
    ):
        raise ValueError("static dependency packages are duplicated or unordered")
    if observed != expected:
        raise ValueError("static dependency archives do not cover selected link plan")
    return value


def capture_system_static_dependencies(
    plan: LlvmNativeLinkPlan, resources: CargoResourceCustody
) -> list[dict[str, Any]]:
    """Capture Debian manufacturing evidence once per owning package."""
    selected = [
        (index, lib)
        for index, lib in enumerate(
            lib for lib in plan.libraries if lib.source == "system"
        )
        if lib.kind == "static"
    ]
    if not selected:
        return []
    if plan.target_os != "linux":
        raise ValueError(
            "static system release provenance requires Linux dpkg manufacturing"
        )
    environment = ProcessEnvironment(os.environ).updated(
        {"LC_ALL": "C.UTF-8", "LANGUAGE": "C"}
    )
    command = resolve_executable(
        "dpkg-query",
        environment=environment,
        cwd=_ROOT,
        label="static system release package manager",
    )

    def query(*args: str) -> str:
        try:
            return _COMMANDS.check_output(
                [str(command), "--admindir=/var/lib/dpkg", *args],
                cwd=_ROOT,
                env=environment,
                text=True,
                encoding="utf-8",
                timeout=30,
            )
        except subprocess.CalledProcessError as exc:
            raise ValueError(
                "cannot capture installed dpkg ownership for static release dependency"
            ) from exc

    packages: dict[str, dict[str, Any]] = {}
    notice_cache: dict[str, dict[str, Any]] = {}
    plan_rows = []
    for index, lib in selected:
        archive_path = lib.path.resolve(strict=True)
        identity = resources.file_identity(lib.path)
        pattern = re.sub(r"([*?\[\\])", r"\\\1", str(archive_path))
        owners = query("--search", "--", pattern).splitlines()
        if len(owners) != 1 or ": " not in owners[0]:
            raise ValueError(
                f"static archive has no unique installed package owner: {archive_path}"
            )
        owner, found_path = owners[0].rsplit(": ", 1)
        if (
            found_path != str(archive_path)
            or re.fullmatch(r"[a-z0-9][a-z0-9+.-]*(?::[a-z0-9][a-z0-9-]*)?", owner)
            is None
        ):
            raise ValueError(f"static archive ownership is ambiguous: {archive_path}")
        if owner not in packages:
            metadata = (
                query(
                    "--show",
                    "--showformat=${Package}\t${Version}\t${Architecture}\t${db:Status-Status}\n",
                    "--",
                    owner,
                )
                .rstrip("\n")
                .split("\t")
            )
            if len(metadata) != 4 or metadata[3] != "installed":
                raise ValueError("static archive owner is not an installed package")
            name, version, architecture, _ = metadata
            if owner not in {name, f"{name}:{architecture}"}:
                raise ValueError("static archive owner differs from package metadata")
            package = {
                "manager": "dpkg",
                "name": name,
                "version": version,
                "architecture": architecture,
            }
            pending = {f"usr/share/doc/{name}/copyright"}
            notices = {}
            while pending:
                path = min(pending)
                pending.remove(path)
                if path not in notice_cache:
                    portable_relative_path(path)
                    identity_notice, data = capture_stable_regular_file(
                        Path("/", path).resolve(strict=True),
                        label="static dependency notice",
                        max_bytes=4 * 1024 * 1024,
                    )
                    notice_cache[path] = {
                        "path": path,
                        "sha256": identity_notice.sha256,
                        "size": identity_notice.size,
                        "content_base64": base64.b64encode(data).decode("ascii"),
                    }
                notice = notice_cache[path]
                notices[path] = notice
                pending.update(_common_notices(notice_bytes(notice)) - notices.keys())
            packages[owner] = {
                "package": package,
                "archives": [],
                "notices": [notices[path] for path in sorted(notices)],
            }
        archive = {
            "path": f"${{compiler/llvm-system/{index}}}",
            "name": lib.name,
            "sha256": identity.sha256,
            "size": identity.size,
        }
        packages[owner]["archives"].append(archive)
        plan_rows.append({**archive, "kind": "static", "source": "system"})
    result = sorted(
        packages.values(),
        key=lambda item: (
            item["package"]["name"],
            item["package"]["architecture"],
            item["package"]["version"],
        ),
    )
    for item in result:
        item["archives"].sort(key=lambda row: row["path"])
    return validate_system_static_dependencies(result, {"libraries": plan_rows})
