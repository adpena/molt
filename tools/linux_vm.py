#!/usr/bin/env python3
"""Molt's Linux VMs (Lima on macOS): one definition, checked, released, applied.

``tools/linux_vm.toml`` defines each VM. This tool is its only consumer:

``check``
    Compares each defined VM's effective configuration (``limactl list
    --json``) with its definition and exits 1 on drift. A running VM whose
    ``lima.yaml`` changed after its hostagent started is drift too: it runs an
    older configuration. For running VMs it also reports the host files their
    virtiofs server holds (one ``lsof``). It runs nothing in a guest.

``release``
    Makes every running VM drop the host files its guest caches, and reports
    the deleted files and bytes its VM process holds before and after. Apple's
    virtiofs server keeps one host file open for each inode the guest caches,
    until the guest evicts the inode (FUSE ``FORGET``); a file the host deletes
    stays allocated until then. The guest action is ``echo 2 >
    /proc/sys/vm/drop_caches``: the kernel frees only unused dentries and
    inodes, so a guest process's open files stay open. It never stops a VM.

``render`` / ``apply``
    ``render`` prints a VM's ``lima.yaml``. ``apply`` validates it with
    ``limactl validate``, then stops the VM, installs it (keeping
    ``lima.yaml.molt-previous``) and starts the VM. It refuses without
    ``--confirm-restart`` and while the guest runs anything but its own
    session and daemon processes.

Lane retirement calls ``release_for_paths`` (``tools/molt_dev_retire.py``)
just before it deletes a checkout: each running VM whose process holds host
files under the checkout, or deleted files, drops its caches first, so the
deletion frees those files at once. With no live Lima hostagent the call
reads one directory and spawns nothing.

Design, prior art and measured costs: ``docs/design/linux_vm_apparatus.md``.
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
import enum
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import tomllib
from typing import Protocol

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.custody_layout import custody_root, git_directories  # noqa: E402
from molt.path_custody import host_path_is_within  # noqa: E402
from molt.process_model import pid_alive  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402

ROOT = _REPO_ROOT
DEFINITIONS_PATH = ROOT / "tools" / "linux_vm.toml"
DEFINITIONS_SCHEMA = 1
CHECK_SCHEMA = "molt.linux-vm.check.v1"
RELEASE_SCHEMA = "molt.linux-vm.release.v1"

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2

# Apple's VM process serves the guest's virtiofs shares and holds its disk.
VZ_COMMAND = "com.apple.Virtualization.VirtualMachine"
# One lsof call lists every VM process's files with link counts. `+L` with no
# number only adds the link-count field; it is not a selection, so `-c` is
# the only selection (lsof ORs selections unless `-a` is given).
LSOF_VZ_FILES = (
    "lsof",
    "-n",
    "-P",
    "+L",
    "-a",
    "-c",
    "com.apple.Virtualization",
    "-F",
    "pcftskn",
)
DROP_GUEST_CACHES = "echo 2 > /proc/sys/vm/drop_caches"
# Prints the guest user's uid, the probe shell's pid, then every process.
IDLE_PROBE = "id -u; echo $$; ps -eo pid=,ppid=,uid=,comm="
# Linux keeps a task's comm in TASK_COMM_LEN (16) bytes with a terminator.
COMM_LENGTH = 15
LIMA_INSTANCE_DISKS = ("disk", "diffdisk")
GUEST_COMMAND_SECONDS = 120.0
LIFECYCLE_SECONDS = 900.0
PREVIOUS_CONFIG = "lima.yaml.molt-previous"
NEXT_CONFIG = "lima.yaml.molt-next"
_SIZE = re.compile(r"^([1-9][0-9]*)(MiB|GiB|TiB)$")
_PARAM_KEY = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
_SIZE_UNITS = {"MiB": 1 << 20, "GiB": 1 << 30, "TiB": 1 << 40}


class DefinitionError(ValueError):
    """``tools/linux_vm.toml`` does not satisfy its schema."""


class LimaError(RuntimeError):
    """A Lima or lsof command failed or printed something this tool cannot read."""


class VmType(enum.StrEnum):
    VZ = "vz"


class Arch(enum.StrEnum):
    AARCH64 = "aarch64"


class MountType(enum.StrEnum):
    VIRTIOFS = "virtiofs"


class MountSource(enum.StrEnum):
    # `molt.custody_layout.custody_root` of the main checkout: `<root>` for a
    # family at `<root>/molt-src` and `<root>/worktrees/<lane>`.
    CUSTODY_ROOT = "custody-root"


class ForwardProto(enum.StrEnum):
    TCP = "tcp"
    UDP = "udp"


class DriftField(enum.StrEnum):
    VM_TYPE = "vm_type"
    ARCH = "arch"
    CPUS = "cpus"
    MEMORY = "memory"
    DISK = "disk"
    IMAGES = "images"
    MOUNT_TYPE = "mount_type"
    MOUNTS = "mounts"
    PORT_FORWARDS = "port_forwards"
    CONTAINERD = "containerd"
    ROSETTA = "rosetta"
    PARAMS = "params"
    RESTART_PENDING = "restart_pending"


# --------------------------------------------------------------------------
# Definitions
# --------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class Size:
    text: str
    bytes: int


@dataclass(frozen=True, slots=True)
class Image:
    location: str
    digest: str


@dataclass(frozen=True, slots=True)
class MountDefinition:
    source: MountSource
    writable: bool


@dataclass(frozen=True, slots=True)
class PortForward:
    guest_port: int
    host_port: int
    proto: ForwardProto


@dataclass(frozen=True, slots=True)
class VmDefinition:
    name: str
    purpose: str
    vm_type: VmType
    arch: Arch
    cpus: int
    memory: Size
    disk: Size
    rosetta: bool
    containerd_user: bool
    mount_type: MountType
    params: tuple[tuple[str, str], ...]
    mounts: tuple[MountDefinition, ...]
    port_forwards: tuple[PortForward, ...]
    image: Image


@dataclass(frozen=True, slots=True)
class Definitions:
    minimum_lima_version: str
    idle_guest_processes: frozenset[str]
    maintenance_guest_processes: frozenset[str]
    vms: Mapping[str, VmDefinition]


def _table(value: object, where: str) -> Mapping[str, object]:
    if not isinstance(value, dict):
        raise DefinitionError(f"{where} must be a table")
    return value


def _keys(table: Mapping[str, object], where: str, required: set[str]) -> None:
    missing = required - table.keys()
    unknown = table.keys() - required
    if missing or unknown:
        raise DefinitionError(
            f"{where}: missing {sorted(missing)}, unknown {sorted(unknown)}"
        )


def _lima_table(value: object, where: str) -> Mapping[str, object]:
    if not isinstance(value, dict):
        raise LimaError(f"{where} is not a mapping in Lima's output")
    return value


def _string(value: object, where: str) -> str:
    if not isinstance(value, str) or not value:
        raise DefinitionError(f"{where} must be a non-empty string")
    return value


def _bool(value: object, where: str) -> bool:
    if not isinstance(value, bool):
        raise DefinitionError(f"{where} must be a boolean")
    return value


def _port(value: object, where: str) -> int:
    if type(value) is not int or not 1 <= value <= 65535:
        raise DefinitionError(f"{where} must be a port from 1 to 65535")
    return value


def _enum[E: enum.StrEnum](kind: type[E], value: object, where: str) -> E:
    try:
        return kind(_string(value, where))
    except ValueError:
        allowed = ", ".join(member.value for member in kind)
        raise DefinitionError(f"{where} must be one of: {allowed}") from None


def _size(value: object, where: str) -> Size:
    match = _SIZE.match(_string(value, where))
    if match is None:
        raise DefinitionError(f"{where} must look like 12GiB (MiB, GiB or TiB)")
    return Size(match.group(0), int(match.group(1)) * _SIZE_UNITS[match.group(2)])


def _names(value: object, where: str) -> frozenset[str]:
    if not isinstance(value, list) or not value:
        raise DefinitionError(f"{where} must be a non-empty list")
    return frozenset(_string(item, where)[:COMM_LENGTH] for item in value)


def _vm(name: str, raw: object) -> VmDefinition:
    where = f"vm.{name}"
    table = _table(raw, where)
    _keys(
        table,
        where,
        {
            "purpose",
            "vm_type",
            "arch",
            "cpus",
            "memory",
            "disk",
            "rosetta",
            "containerd_user",
            "mount_type",
            "params",
            "mounts",
            "port_forwards",
            "image",
        },
    )
    cpus = table["cpus"]
    if type(cpus) is not int or cpus < 1:
        raise DefinitionError(f"{where}.cpus must be a positive integer")
    params = _table(table["params"], f"{where}.params")
    for key, value in params.items():
        if not _PARAM_KEY.match(key):
            raise DefinitionError(f"{where}.params key {key!r} is not an identifier")
        _string(value, f"{where}.params.{key}")
    mounts_raw = table["mounts"]
    if not isinstance(mounts_raw, list):
        raise DefinitionError(f"{where}.mounts must be a list")
    mounts = []
    for index, mount_raw in enumerate(mounts_raw):
        mount_where = f"{where}.mounts[{index}]"
        mount = _table(mount_raw, mount_where)
        _keys(mount, mount_where, {"source", "writable"})
        mounts.append(
            MountDefinition(
                _enum(MountSource, mount["source"], f"{mount_where}.source"),
                _bool(mount["writable"], f"{mount_where}.writable"),
            )
        )
    if len({mount.source for mount in mounts}) != len(mounts):
        raise DefinitionError(f"{where}.mounts names one source twice")
    forwards_raw = table["port_forwards"]
    if not isinstance(forwards_raw, list):
        raise DefinitionError(f"{where}.port_forwards must be a list")
    forwards = []
    for index, forward_raw in enumerate(forwards_raw):
        forward_where = f"{where}.port_forwards[{index}]"
        forward = _table(forward_raw, forward_where)
        _keys(forward, forward_where, {"guest_port", "host_port", "proto"})
        forwards.append(
            PortForward(
                _port(forward["guest_port"], f"{forward_where}.guest_port"),
                _port(forward["host_port"], f"{forward_where}.host_port"),
                _enum(ForwardProto, forward["proto"], f"{forward_where}.proto"),
            )
        )
    image = _table(table["image"], f"{where}.image")
    _keys(image, f"{where}.image", {"location", "digest"})
    digest = _string(image["digest"], f"{where}.image.digest")
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise DefinitionError(f"{where}.image.digest must be sha256:<64 hex>")
    return VmDefinition(
        name=name,
        purpose=_string(table["purpose"], f"{where}.purpose"),
        vm_type=_enum(VmType, table["vm_type"], f"{where}.vm_type"),
        arch=_enum(Arch, table["arch"], f"{where}.arch"),
        cpus=cpus,
        memory=_size(table["memory"], f"{where}.memory"),
        disk=_size(table["disk"], f"{where}.disk"),
        rosetta=_bool(table["rosetta"], f"{where}.rosetta"),
        containerd_user=_bool(table["containerd_user"], f"{where}.containerd_user"),
        mount_type=_enum(MountType, table["mount_type"], f"{where}.mount_type"),
        params=tuple(sorted((key, str(value)) for key, value in params.items())),
        mounts=tuple(mounts),
        port_forwards=tuple(forwards),
        image=Image(_string(image["location"], f"{where}.image.location"), digest),
    )


def load_definitions(path: Path = DEFINITIONS_PATH) -> Definitions:
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise DefinitionError(f"cannot read {path}: {exc}") from exc
    _keys(
        raw,
        str(path),
        {
            "schema",
            "minimum_lima_version",
            "idle_guest_processes",
            "maintenance_guest_processes",
            "vm",
        },
    )
    if raw["schema"] != DEFINITIONS_SCHEMA:
        raise DefinitionError(f"{path}: schema must be {DEFINITIONS_SCHEMA}")
    vms = _table(raw["vm"], "vm")
    if not vms:
        raise DefinitionError(f"{path} defines no VM")
    return Definitions(
        minimum_lima_version=_string(
            raw["minimum_lima_version"], "minimum_lima_version"
        ),
        idle_guest_processes=_names(
            raw["idle_guest_processes"], "idle_guest_processes"
        ),
        maintenance_guest_processes=_names(
            raw["maintenance_guest_processes"], "maintenance_guest_processes"
        ),
        vms={name: _vm(name, vm) for name, vm in vms.items()},
    )


def family_custody_root(repo_root: Path = ROOT) -> Path:
    """The custody root of this checkout's family, read through Git metadata.

    A linked worktree's common directory is the main checkout's ``.git``, so
    every member of one family resolves the same root without a Git process.
    """
    directories = git_directories(repo_root)
    main = repo_root
    if directories is not None and directories[1].name == ".git":
        main = directories[1].parent
    return custody_root(main)


def mount_location(mount: MountDefinition, *, custody: Path) -> Path:
    match mount.source:
        case MountSource.CUSTODY_ROOT:
            return custody


# --------------------------------------------------------------------------
# Rendering
# --------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class PortRule:
    """One Lima port rule, with every key that decides what it forwards."""

    guest_ip: str | None
    guest_ip_must_be_zero: bool
    guest_port_range: tuple[int, int] | None
    host_ip: str | None
    host_port_range: tuple[int, int] | None
    proto: str | None
    ignore: bool

    def lima(self) -> dict[str, object]:
        rule: dict[str, object] = {
            "guestIP": self.guest_ip,
            "guestIPMustBeZero": self.guest_ip_must_be_zero,
            "guestPortRange": list(self.guest_port_range or ()),
            "hostIP": self.host_ip,
            "hostPortRange": list(self.host_port_range or ()),
            "proto": self.proto,
        }
        if self.ignore:
            rule["ignore"] = True
        return rule

    @classmethod
    def from_lima(cls, raw: object) -> PortRule:
        rule = _lima_table(raw, "portForwards entry")

        def port_range(key: str) -> tuple[int, int] | None:
            value = rule.get(key)
            if isinstance(value, list) and len(value) == 2:
                return (int(value[0]), int(value[1]))
            return None

        def text(key: str) -> str | None:
            value = rule.get(key)
            return value if isinstance(value, str) and value else None

        return cls(
            guest_ip=text("guestIP"),
            guest_ip_must_be_zero=rule.get("guestIPMustBeZero") is True,
            guest_port_range=port_range("guestPortRange"),
            host_ip=text("hostIP"),
            host_port_range=port_range("hostPortRange"),
            proto=text("proto"),
            ignore=rule.get("ignore") is True,
        )

    def describe(self) -> str:
        action = (
            "ignore" if self.ignore else f"-> {self.host_ip}:{self.host_port_range}"
        )
        return f"{self.proto} {self.guest_ip}:{self.guest_port_range} {action}"


# Lima appends a rule that forwards every guest port on 127.0.0.1 and checks
# rules in order, first match wins (Lima default.yaml, pkg/portfwd). This last
# rule matches every guest address and port first, so nothing undeclared is
# forwarded.
IGNORE_EVERY_GUEST_PORT = PortRule(
    guest_ip="0.0.0.0",
    guest_ip_must_be_zero=False,
    guest_port_range=(1, 65535),
    host_ip="127.0.0.1",
    host_port_range=(1, 65535),
    proto="any",
    ignore=True,
)


def port_rules(vm: VmDefinition) -> tuple[PortRule, ...]:
    declared = tuple(
        PortRule(
            guest_ip="127.0.0.1",
            guest_ip_must_be_zero=False,
            guest_port_range=(forward.guest_port, forward.guest_port),
            host_ip="127.0.0.1",
            host_port_range=(forward.host_port, forward.host_port),
            proto=forward.proto.value,
            ignore=False,
        )
        for forward in vm.port_forwards
    )
    return (*declared, IGNORE_EVERY_GUEST_PORT)


def lima_config(
    vm: VmDefinition, *, custody: Path, minimum_lima_version: str
) -> dict[str, object]:
    config: dict[str, object] = {
        "minimumLimaVersion": minimum_lima_version,
        "vmType": vm.vm_type.value,
        "arch": vm.arch.value,
        "cpus": vm.cpus,
        "memory": vm.memory.text,
        "disk": vm.disk.text,
        "images": [
            {
                "location": vm.image.location,
                "arch": vm.arch.value,
                "digest": vm.image.digest,
            }
        ],
        "mountType": vm.mount_type.value,
        "mounts": [
            {
                "location": str(mount_location(mount, custody=custody)),
                "writable": mount.writable,
            }
            for mount in vm.mounts
        ],
        "portForwards": [rule.lima() for rule in port_rules(vm)],
        "containerd": {"system": False, "user": vm.containerd_user},
    }
    if vm.params:
        config["param"] = dict(vm.params)
    if vm.rosetta:
        config["vmOpts"] = {"vz": {"rosetta": {"enabled": True, "binfmt": True}}}
    return config


def _yaml_scalar(value: object) -> str:
    # JSON strings are valid YAML double-quoted scalars.
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return json.dumps(value)
    raise TypeError(f"no YAML form for {value!r}")


def _yaml_lines(value: Mapping[str, object], indent: int) -> list[str]:
    pad = "  " * indent
    lines: list[str] = []
    for key, item in value.items():
        if isinstance(item, dict):
            lines.append(f"{pad}{key}:")
            lines.extend(_yaml_lines(item, indent + 1))
        elif isinstance(item, list) and all(isinstance(x, dict) for x in item):
            if not item:
                lines.append(f"{pad}{key}: []")
                continue
            lines.append(f"{pad}{key}:")
            for entry in item:
                entry_lines = _yaml_lines(entry, indent + 2)
                first = entry_lines[0][len(pad) + 4 :]
                lines.append(f"{pad}  - {first}")
                lines.extend(entry_lines[1:])
        elif isinstance(item, list):
            lines.append(f"{pad}{key}: [{', '.join(_yaml_scalar(x) for x in item)}]")
        else:
            lines.append(f"{pad}{key}: {_yaml_scalar(item)}")
    return lines


def render(vm: VmDefinition, *, custody: Path, minimum_lima_version: str) -> str:
    header = (
        f"# Generated by tools/linux_vm.py from tools/linux_vm.toml (vm.{vm.name}).\n"
        "# Edit the definition and run `tools/linux_vm.py apply`; do not edit this file.\n"
    )
    config = lima_config(vm, custody=custody, minimum_lima_version=minimum_lima_version)
    return header + "\n".join(_yaml_lines(config, 0)) + "\n"


# --------------------------------------------------------------------------
# Host commands
# --------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class Completed:
    returncode: int
    stdout: str
    stderr: str


class Host(Protocol):
    def which(self, name: str) -> str | None: ...

    def run(self, argv: Sequence[str], *, timeout: float) -> Completed: ...


class LocalHost:
    """Runs host commands under Molt's guarded command executor.

    The executor binds on the first command (about 1 ms), so a caller that
    finds no VM pays nothing for it.
    """

    def __init__(self, environment: Mapping[str, str]) -> None:
        self.environment = dict(environment)
        self._commands: CommandExecutor | None = None

    def which(self, name: str) -> str | None:
        return shutil.which(name, path=self.environment.get("PATH"))

    def run(self, argv: Sequence[str], *, timeout: float) -> Completed:
        if self._commands is None:
            self._commands = CommandExecutor.for_file(__file__)
        try:
            completed = self._commands.run(
                list(argv),
                cwd=ROOT,
                env=self.environment,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout,
                check=False,
            )
        except subprocess.TimeoutExpired:
            return Completed(124, "", f"{argv[0]} timed out after {timeout:.0f} s\n")
        return Completed(completed.returncode, completed.stdout, completed.stderr)


def lima_home(environment: Mapping[str, str]) -> Path:
    """Lima's own rule: ``$LIMA_HOME``, else ``~/.lima``."""
    configured = environment.get("LIMA_HOME")
    if configured:
        return Path(configured)
    return Path(environment.get("HOME") or Path.home()) / ".lima"


# --------------------------------------------------------------------------
# Effective configuration
# --------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class Instance:
    name: str
    status: str
    directory: Path
    vm_type: str
    arch: str
    cpus: int
    memory: int
    disk: int
    config: Mapping[str, object]

    @property
    def running(self) -> bool:
        return self.status == "Running"

    def mounts(self) -> tuple[tuple[Path, Path, bool], ...]:
        rows = []
        for raw in self.config.get("mounts") or ():
            mount = _lima_table(raw, f"{self.name} mount")
            location = Path(str(mount.get("location")))
            point = mount.get("mountPoint")
            rows.append(
                (
                    location,
                    Path(point) if isinstance(point, str) and point else location,
                    mount.get("writable") is True,
                )
            )
        return tuple(rows)

    def mount_type(self) -> str | None:
        declared = self.config.get("mountType")
        if isinstance(declared, str) and declared:
            return declared
        # Lima's vz driver fills an undeclared mountType with virtiofs
        # (pkg/driver/vz/vz_driver_darwin.go, v2.2.1); `limactl list` does not.
        if self.vm_type == VmType.VZ.value:
            return MountType.VIRTIOFS.value
        return None


def _instance(line: str) -> Instance:
    row = _lima_table(json.loads(line), "limactl list entry")
    config = row.get("config")
    return Instance(
        name=str(row["name"]),
        status=str(row.get("status", "")),
        directory=Path(str(row["dir"])),
        vm_type=str(row.get("vmType", "")),
        arch=str(row.get("arch", "")),
        cpus=int(row.get("cpus") or 0),
        memory=int(row.get("memory") or 0),
        disk=int(row.get("disk") or 0),
        config=config if isinstance(config, dict) else {},
    )


def list_instances(host: Host, names: Sequence[str] = ()) -> tuple[Instance, ...]:
    """Every Lima instance (or the named ones) from one ``limactl list --json``."""
    completed = host.run(["limactl", "list", "--json", *names], timeout=60.0)
    if completed.returncode != 0:
        raise LimaError(f"limactl list failed: {completed.stderr.strip()}")
    try:
        return tuple(
            _instance(line) for line in completed.stdout.splitlines() if line.strip()
        )
    except (ValueError, KeyError, TypeError) as exc:
        raise LimaError(f"cannot read `limactl list --json`: {exc}") from exc


@dataclass(frozen=True, slots=True)
class Drift:
    field: DriftField
    declared: str
    effective: str


def _resolved(path: Path) -> Path:
    return path.expanduser().resolve(strict=False)


def _format_bytes(count: int) -> str:
    for unit, scale in (("TiB", 1 << 40), ("GiB", 1 << 30), ("MiB", 1 << 20)):
        if count >= scale and count % scale == 0:
            return f"{count // scale}{unit}"
    return f"{count} B"


def _mount_rows(
    rows: Iterable[tuple[Path, Path, bool]],
) -> tuple[tuple[str, str, bool], ...]:
    return tuple(
        (str(_resolved(location)), str(_resolved(point)), writable)
        for location, point, writable in rows
    )


def _describe_mounts(rows: tuple[tuple[str, str, bool], ...]) -> str:
    if not rows:
        return "none"
    return ", ".join(
        f"{location}{'' if point == location else ' at ' + point}"
        f" ({'rw' if writable else 'ro'})"
        for location, point, writable in rows
    )


def restart_pending(instance: Instance) -> bool:
    """A running VM whose lima.yaml changed after its hostagent started."""
    if not instance.running:
        return False
    try:
        started = (instance.directory / "ha.pid").stat().st_mtime
        configured = (instance.directory / "lima.yaml").stat().st_mtime
    except OSError:
        return False
    return configured > started


def compare(
    vm: VmDefinition, instance: Instance, *, custody: Path
) -> tuple[Drift, ...]:
    """Every field where ``instance`` does not run ``vm``'s definition."""
    drift: list[Drift] = []

    def differ(field_: DriftField, declared: object, effective: object) -> None:
        if declared != effective:
            drift.append(Drift(field_, str(declared), str(effective)))

    config = instance.config
    differ(DriftField.VM_TYPE, vm.vm_type.value, instance.vm_type)
    differ(DriftField.ARCH, vm.arch.value, instance.arch)
    differ(DriftField.CPUS, vm.cpus, instance.cpus)
    if vm.memory.bytes != instance.memory:
        drift.append(
            Drift(DriftField.MEMORY, vm.memory.text, _format_bytes(instance.memory))
        )
    if vm.disk.bytes != instance.disk:
        drift.append(Drift(DriftField.DISK, vm.disk.text, _format_bytes(instance.disk)))
    images = tuple(
        (str(image.get("location")), str(image.get("arch")), str(image.get("digest")))
        for image in (_lima_table(raw, "image") for raw in (config.get("images") or ()))
    )
    expected_images = ((vm.image.location, vm.arch.value, vm.image.digest),)
    if images != expected_images:
        drift.append(
            Drift(
                DriftField.IMAGES,
                f"{vm.image.location} ({vm.image.digest})",
                f"{len(images)} image entries",
            )
        )
    differ(DriftField.MOUNT_TYPE, vm.mount_type.value, instance.mount_type())
    expected_mounts = _mount_rows(
        (
            mount_location(mount, custody=custody),
            mount_location(mount, custody=custody),
            mount.writable,
        )
        for mount in vm.mounts
    )
    effective_mounts = _mount_rows(instance.mounts())
    if effective_mounts != expected_mounts:
        drift.append(
            Drift(
                DriftField.MOUNTS,
                _describe_mounts(expected_mounts),
                _describe_mounts(effective_mounts),
            )
        )
    rules = tuple(PortRule.from_lima(raw) for raw in config.get("portForwards") or ())
    expected_rules = port_rules(vm)
    if rules != expected_rules:
        drift.append(
            Drift(
                DriftField.PORT_FORWARDS,
                "; ".join(rule.describe() for rule in expected_rules),
                "; ".join(rule.describe() for rule in rules)
                or "none (Lima forwards every guest port on 127.0.0.1)",
            )
        )
    containerd = config.get("containerd")
    containerd = containerd if isinstance(containerd, dict) else {}
    differ(
        DriftField.CONTAINERD,
        f"system={False} user={vm.containerd_user}",
        f"system={containerd.get('system') is True} "
        f"user={containerd.get('user') is True}",
    )
    vm_opts = config.get("vmOpts")
    vz = vm_opts.get("vz") if isinstance(vm_opts, dict) else None
    rosetta = vz.get("rosetta") if isinstance(vz, dict) else None
    rosetta = rosetta if isinstance(rosetta, dict) else {}
    differ(
        DriftField.ROSETTA,
        f"enabled={vm.rosetta} binfmt={vm.rosetta}",
        f"enabled={rosetta.get('enabled') is True} "
        f"binfmt={rosetta.get('binfmt') is True}",
    )
    params = config.get("param")
    params = params if isinstance(params, dict) else {}
    differ(
        DriftField.PARAMS,
        dict(vm.params),
        {str(key): str(value) for key, value in sorted(params.items())},
    )
    if restart_pending(instance):
        drift.append(
            Drift(
                DriftField.RESTART_PENDING,
                "running the configuration in lima.yaml",
                "lima.yaml changed after the VM started",
            )
        )
    return tuple(drift)


# --------------------------------------------------------------------------
# Host files held by VM processes
# --------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class Held:
    """What one VM process holds open, from one lsof snapshot."""

    pid: int
    deleted_files: int
    deleted_bytes: int
    open_under_mounts: int
    # Open files under the paths the caller is about to delete.
    open_under_paths: int = 0

    def json(self) -> dict[str, int]:
        return {
            "pid": self.pid,
            "deleted_files": self.deleted_files,
            "deleted_bytes": self.deleted_bytes,
            "open_under_mounts": self.open_under_mounts,
            "open_under_paths": self.open_under_paths,
        }


@dataclass(slots=True)
class _OpenFile:
    fd: str
    kind: str = ""
    size: int = 0
    links: int | None = None
    name: str = ""


@dataclass(slots=True)
class _Process:
    pid: int
    command: str = ""
    files: list[_OpenFile] = field(default_factory=list)


def _lsof_processes(text: str) -> list[_Process]:
    processes: list[_Process] = []
    current: _OpenFile | None = None
    for line in text.splitlines():
        if not line:
            continue
        tag, value = line[0], line[1:]
        if tag == "p":
            processes.append(_Process(int(value)))
            current = None
        elif not processes:
            raise LimaError(f"lsof field {line!r} precedes any process")
        elif tag == "c":
            processes[-1].command = value
        elif tag == "f":
            current = _OpenFile(value)
            processes[-1].files.append(current)
        elif current is None:
            continue
        elif tag == "t":
            current.kind = value
        elif tag == "s":
            current.size = int(value)
        elif tag == "k":
            current.links = int(value)
        elif tag == "n":
            current.name = value
    return processes


def _under(name: str, roots: Sequence[str]) -> bool:
    return any(
        name == root or name.startswith(root.rstrip("/") + "/") for root in roots
    )


def held_files(
    host: Host, instances: Sequence[Instance], *, paths: Sequence[Path] = ()
) -> dict[str, Held]:
    """Map each running instance to its VM process's held files.

    A VM process belongs to the instance whose disk image it holds open. A
    held deleted file is a regular file on a numbered descriptor with no links
    left: the share's pins. Mapped executables (``txt``) are the process's own.
    A deleted file's path is often partial, so every one counts. ``paths``
    adds the count of open files under them.
    """
    if not instances:
        return {}
    completed = host.run(list(LSOF_VZ_FILES), timeout=60.0)
    # lsof exits 1 when no process matches.
    if completed.returncode not in (0, 1):
        raise LimaError(f"lsof failed: {completed.stderr.strip()}")
    disks = {
        str(_resolved(instance.directory / disk)): instance
        for instance in instances
        for disk in LIMA_INSTANCE_DISKS
    }
    disk_suffixes = tuple(f"/{disk}" for disk in LIMA_INSTANCE_DISKS)
    held: dict[str, Held] = {}
    for process in _lsof_processes(completed.stdout):
        if process.command != VZ_COMMAND:
            continue
        owner = next(
            (
                disks[path]
                for path in (
                    str(_resolved(Path(item.name)))
                    for item in process.files
                    if item.name.endswith(disk_suffixes)
                )
                if path in disks
            ),
            None,
        )
        if owner is None:
            continue
        # lsof prints the kernel's resolved paths; compare them as text.
        mounts = [str(_resolved(location)) for location, _point, _rw in owner.mounts()]
        watched = [str(_resolved(path)) for path in paths]
        deleted = [
            item
            for item in process.files
            if item.fd.isdigit() and item.kind == "REG" and item.links == 0
        ]
        # The instance's own disk and logs lie under a mount of the home.
        own = str(_resolved(owner.directory)).rstrip("/") + "/"
        opened = [
            item.name
            for item in process.files
            if item.fd.isdigit() and item.links and not item.name.startswith(own)
        ]
        held[owner.name] = Held(
            pid=process.pid,
            deleted_files=len(deleted),
            deleted_bytes=sum(item.size for item in deleted),
            open_under_mounts=sum(1 for name in opened if _under(name, mounts)),
            open_under_paths=sum(1 for name in opened if _under(name, watched)),
        )
    return held


# --------------------------------------------------------------------------
# Release
# --------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class Release:
    name: str
    before: Held | None
    after: Held | None
    seconds: float
    error: str | None

    def json(self) -> dict[str, object]:
        return {
            "name": self.name,
            "before": None if self.before is None else self.before.json(),
            "after": None if self.after is None else self.after.json(),
            "seconds": round(self.seconds, 3),
            "error": self.error,
        }


def drop_guest_caches(host: Host, name: str) -> Completed:
    return host.run(
        [
            "limactl",
            "shell",
            "--workdir",
            "/",
            name,
            "sudo",
            "-n",
            "sh",
            "-c",
            DROP_GUEST_CACHES,
        ],
        timeout=GUEST_COMMAND_SECONDS,
    )


def release(
    host: Host,
    instances: Sequence[Instance],
    *,
    before: Mapping[str, Held] | None = None,
    paths: Sequence[Path] = (),
) -> tuple[Release, ...]:
    """Make each running vz instance drop its guest caches; measure around it.

    ``before`` reuses a snapshot the caller just took of these instances with
    the same ``paths``.
    """
    running = [
        instance
        for instance in instances
        if instance.running and instance.vm_type == VmType.VZ.value
    ]
    if before is None:
        before = held_files(host, running, paths=paths)
    outcomes: list[tuple[Instance, float, str | None]] = []
    for instance in running:
        if instance.name not in before:
            outcomes.append((instance, 0.0, "no VM process holds this instance's disk"))
            continue
        start = time.monotonic()
        completed = drop_guest_caches(host, instance.name)
        seconds = time.monotonic() - start
        error = None
        if completed.returncode != 0:
            error = (
                f"drop_caches failed (exit {completed.returncode}): "
                f"{completed.stderr.strip()}"
            )
        outcomes.append((instance, seconds, error))
    after = held_files(host, running, paths=paths)
    return tuple(
        Release(
            instance.name,
            before.get(instance.name),
            after.get(instance.name),
            seconds,
            error,
        )
        for instance, seconds, error in outcomes
    )


def live_hostagents(home: Path) -> tuple[Path, ...]:
    """Instance directories whose Lima hostagent is alive; spawns nothing."""
    try:
        candidates = sorted(home.iterdir())
    except OSError:
        return ()
    live = []
    for directory in candidates:
        if directory.name.startswith(("_", ".")):
            continue
        try:
            pid = int((directory / "ha.pid").read_text(encoding="utf-8").strip())
        except (OSError, ValueError):
            continue
        try:
            alive = pid_alive(pid)
        except OSError:
            alive = True  # An unknown observation is not proven death.
        if alive:
            live.append(directory)
    return tuple(live)


def release_for_paths(
    paths: Sequence[Path], *, environment: Mapping[str, str], host: Host
) -> tuple[Release, ...]:
    """Before Molt deletes ``paths``: release the VMs that would pin them.

    A host file a guest caches stays allocated after its deletion until the
    guest evicts the inode, so each running VM that mounts a path and whose
    process holds files under it (or already holds deleted files) drops its
    caches first; the deletion then frees the files at once. Files a guest
    process has open stay open and are reported.

    Costs nothing beyond one directory listing when no hostagent runs. With
    one running, it lists instances (~25 ms) and, only when a running VM
    mounts a path, reads the VM processes' files (~50 ms) and drops the caches
    of each VM that holds files there (~70 ms).
    """
    if not live_hostagents(lima_home(environment)) or host.which("limactl") is None:
        return ()
    watched = tuple(_resolved(path) for path in paths)
    covering = [
        instance
        for instance in list_instances(host)
        if instance.running
        and instance.vm_type == VmType.VZ.value
        and any(
            host_path_is_within(path, location)
            for path in watched
            for location, _point, _writable in instance.mounts()
        )
    ]
    if not covering:
        return ()
    held = held_files(host, covering, paths=watched)
    holding = [
        instance
        for instance in covering
        if instance.name in held
        and (held[instance.name].open_under_paths or held[instance.name].deleted_files)
    ]
    if not holding:
        return ()
    return release(host, holding, before=held, paths=watched)


# --------------------------------------------------------------------------
# Apply
# --------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class GuestProcess:
    pid: int
    ppid: int
    uid: int
    comm: str


def busy_guest_processes(
    probe_output: str, definitions: Definitions
) -> tuple[GuestProcess, ...]:
    """The guest processes that make a restart unsafe.

    The probe's own shell, its ancestors (this session) and its children are
    excluded. Every other process of the guest user must be a session or
    daemon process, and no maintenance process of any user may run.
    """
    lines = probe_output.splitlines()
    if len(lines) < 2:
        raise LimaError(f"unreadable guest process probe: {probe_output!r}")
    try:
        guest_uid = int(lines[0].strip())
        shell = int(lines[1].strip())
    except ValueError as exc:
        raise LimaError(f"unreadable guest process probe: {exc}") from exc
    processes: dict[int, GuestProcess] = {}
    for line in lines[2:]:
        parts = line.split(maxsplit=3)
        if len(parts) != 4:
            raise LimaError(f"unreadable guest process line: {line!r}")
        process = GuestProcess(int(parts[0]), int(parts[1]), int(parts[2]), parts[3])
        processes[process.pid] = process
    if shell not in processes:
        raise LimaError("the guest process probe does not list its own shell")
    session = {shell}
    cursor = processes[shell].ppid
    while cursor in processes and cursor not in session:
        session.add(cursor)
        cursor = processes[cursor].ppid
    busy = []
    for process in processes.values():
        if process.pid in session or process.ppid == shell:
            continue
        comm = process.comm[:COMM_LENGTH]
        if comm in definitions.maintenance_guest_processes or (
            process.uid == guest_uid and comm not in definitions.idle_guest_processes
        ):
            busy.append(process)
    return tuple(busy)


def _say(message: str) -> None:
    print(message, flush=True)


def apply(
    name: str,
    *,
    definitions: Definitions,
    host: Host,
    custody: Path,
    confirm_restart: bool,
) -> int:
    vm = definitions.vms.get(name)
    if vm is None:
        _say(f"REFUSED: {name} is not defined in tools/linux_vm.toml")
        return EXIT_USAGE
    if not confirm_restart:
        _say(
            f"REFUSED: apply restarts {name}; pass --confirm-restart after "
            "checking that no one else uses it"
        )
        return EXIT_FAIL
    instances = list_instances(host, [name])
    if not instances:
        _say(
            f"REFUSED: {name} does not exist; create it from "
            f"`tools/linux_vm.py render {name}` with `limactl create`"
        )
        return EXIT_FAIL
    instance = instances[0]
    if (instance.vm_type, instance.arch) != (vm.vm_type.value, vm.arch.value):
        _say(
            f"REFUSED: {name} runs {instance.vm_type}/{instance.arch}; the "
            f"definition needs {vm.vm_type.value}/{vm.arch.value}, which only a "
            "new VM can have"
        )
        return EXIT_FAIL
    config_path = instance.directory / "lima.yaml"
    next_path = instance.directory / NEXT_CONFIG
    next_path.write_text(
        render(
            vm, custody=custody, minimum_lima_version=definitions.minimum_lima_version
        ),
        encoding="utf-8",
    )
    validated = host.run(["limactl", "validate", str(next_path)], timeout=60.0)
    if validated.returncode != 0:
        next_path.unlink()
        _say(
            f"REFUSED: limactl rejects the rendered config: {validated.stderr.strip()}"
        )
        return EXIT_FAIL
    was_running = instance.running
    if was_running:
        probe = host.run(
            ["limactl", "shell", "--workdir", "/", name, "sh", "-c", IDLE_PROBE],
            timeout=GUEST_COMMAND_SECONDS,
        )
        if probe.returncode != 0:
            next_path.unlink()
            _say(f"REFUSED: cannot list {name}'s processes: {probe.stderr.strip()}")
            return EXIT_FAIL
        busy = busy_guest_processes(probe.stdout, definitions)
        if busy:
            next_path.unlink()
            listed = ", ".join(f"{p.comm} (pid {p.pid}, uid {p.uid})" for p in busy)
            _say(f"REFUSED: {name} is busy: {listed}")
            return EXIT_FAIL
        stopped = host.run(["limactl", "stop", name], timeout=LIFECYCLE_SECONDS)
        if stopped.returncode != 0:
            next_path.unlink()
            _say(f"FAILED: limactl stop {name}: {stopped.stderr.strip()}")
            return EXIT_FAIL
    shutil.copy2(config_path, instance.directory / PREVIOUS_CONFIG)
    os.replace(next_path, config_path)
    _say(f"installed {config_path} (previous: {PREVIOUS_CONFIG})")
    if was_running:
        started = host.run(
            ["limactl", "start", "--tty=false", name], timeout=LIFECYCLE_SECONDS
        )
        if started.returncode != 0:
            _say(
                f"FAILED: limactl start {name}: {started.stderr.strip()}; the "
                f"previous config is {instance.directory / PREVIOUS_CONFIG}"
            )
            return EXIT_FAIL
    after = list_instances(host, [name])
    drift = compare(vm, after[0], custody=custody) if after else ()
    for item in drift:
        _say(f"  drift {item.field}: declared {item.declared}; runs {item.effective}")
    if drift:
        return EXIT_FAIL
    state = "restarted" if was_running else "stopped; it starts"
    _say(f"OK: {name} {state} with its definition")
    return EXIT_OK


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def _describe_held(held: Held | None) -> str:
    if held is None:
        return "no VM process"
    return (
        f"{held.deleted_files} deleted files ({held.deleted_bytes} bytes), "
        f"{held.open_under_mounts} open under its mounts"
    )


def check(
    *,
    definitions: Definitions,
    host: Host,
    custody: Path,
    names: Sequence[str],
    as_json: bool,
) -> int:
    selected = list(names or definitions.vms)
    unknown = [name for name in selected if name not in definitions.vms]
    if unknown:
        _say(f"not defined in tools/linux_vm.toml: {', '.join(unknown)}")
        return EXIT_USAGE
    lima_present = host.which("limactl") is not None
    instances = list_instances(host) if lima_present else ()
    by_name = {instance.name: instance for instance in instances}
    defined = [by_name[name] for name in selected if name in by_name]
    held = held_files(
        host,
        [i for i in defined if i.running and i.vm_type == VmType.VZ.value],
    )
    rows = []
    drifted = False
    for name in selected:
        instance = by_name.get(name)
        drift = (
            ()
            if instance is None
            else compare(definitions.vms[name], instance, custody=custody)
        )
        drifted = drifted or bool(drift)
        rows.append(
            {
                "name": name,
                "status": "absent" if instance is None else instance.status,
                "drift": [
                    {
                        "field": str(d.field),
                        "declared": d.declared,
                        "effective": d.effective,
                    }
                    for d in drift
                ],
                "held": None if name not in held else held[name].json(),
            }
        )
    undeclared = sorted(set(by_name) - set(definitions.vms))
    if as_json:
        print(
            json.dumps(
                {
                    "schema": CHECK_SCHEMA,
                    "lima": lima_present,
                    "vms": rows,
                    "undeclared": undeclared,
                },
                indent=2,
            )
        )
        return EXIT_FAIL if drifted else EXIT_OK
    if not lima_present:
        _say("limactl is not on PATH: no Lima VM to check")
    for row in rows:
        state = "DRIFT" if row["drift"] else "ok"
        _say(f"{row['name']}: {row['status']}, {state}")
        for item in row["drift"]:
            _say(
                f"  {item['field']}: declared {item['declared']}; "
                f"effective {item['effective']}"
            )
        if row["name"] in held:
            _say(f"  holds {_describe_held(held[row['name']])}")
    for name in undeclared:
        _say(f"{name}: not defined by Molt, not checked")
    if drifted:
        _say("apply a definition with: tools/linux_vm.py apply <vm> --confirm-restart")
    return EXIT_FAIL if drifted else EXIT_OK


def release_command(*, host: Host, names: Sequence[str], as_json: bool) -> int:
    if host.which("limactl") is None:
        _say("limactl is not on PATH: no Lima VM to release")
        return EXIT_OK
    instances = [
        instance
        for instance in list_instances(host)
        if not names or instance.name in names
    ]
    missing = sorted(set(names) - {instance.name for instance in instances})
    if missing:
        _say(f"no Lima instance named: {', '.join(missing)}")
        return EXIT_USAGE
    results = release(host, instances)
    if as_json:
        print(
            json.dumps(
                {"schema": RELEASE_SCHEMA, "vms": [r.json() for r in results]},
                indent=2,
            )
        )
    else:
        if not results:
            _say("no running vz VM")
        for result in results:
            _say(f"{result.name}: dropped guest caches in {result.seconds:.3f} s")
            _say(f"  before: {_describe_held(result.before)}")
            _say(f"  after:  {_describe_held(result.after)}")
            if result.error:
                _say(f"  ERROR: {result.error}")
    return EXIT_FAIL if any(r.error for r in results) else EXIT_OK


def main(
    argv: Sequence[str] | None = None,
    *,
    host: Host | None = None,
    definitions_path: Path = DEFINITIONS_PATH,
    custody: Path | None = None,
) -> int:
    parser = argparse.ArgumentParser(
        description="Check, release and apply Molt's Lima VM definitions."
    )
    commands = parser.add_subparsers(dest="command", required=True)
    check_parser = commands.add_parser("check", help="report drift (exit 1)")
    check_parser.add_argument("--vm", action="append", default=[])
    check_parser.add_argument("--json", action="store_true")
    release_parser = commands.add_parser(
        "release", help="make running VMs drop the host files they pin"
    )
    release_parser.add_argument("--vm", action="append", default=[])
    release_parser.add_argument("--json", action="store_true")
    render_parser = commands.add_parser("render", help="print a VM's lima.yaml")
    render_parser.add_argument("vm")
    apply_parser = commands.add_parser(
        "apply", help="install a VM's definition and restart it"
    )
    apply_parser.add_argument("vm")
    apply_parser.add_argument("--confirm-restart", action="store_true")
    args = parser.parse_args(argv)
    try:
        definitions = load_definitions(definitions_path)
    except DefinitionError as exc:
        _say(f"ERROR: {exc}")
        return EXIT_USAGE
    host = host or LocalHost(os.environ)
    custody = custody or family_custody_root()
    try:
        match args.command:
            case "check":
                return check(
                    definitions=definitions,
                    host=host,
                    custody=custody,
                    names=args.vm,
                    as_json=args.json,
                )
            case "release":
                return release_command(host=host, names=args.vm, as_json=args.json)
            case "render":
                vm = definitions.vms.get(args.vm)
                if vm is None:
                    _say(f"ERROR: {args.vm} is not defined in tools/linux_vm.toml")
                    return EXIT_USAGE
                sys.stdout.write(
                    render(
                        vm,
                        custody=custody,
                        minimum_lima_version=definitions.minimum_lima_version,
                    )
                )
                return EXIT_OK
            case "apply":
                return apply(
                    args.vm,
                    definitions=definitions,
                    host=host,
                    custody=custody,
                    confirm_restart=args.confirm_restart,
                )
    except LimaError as exc:
        _say(f"ERROR: {exc}")
        return EXIT_FAIL
    raise AssertionError(f"unhandled command {args.command}")


if __name__ == "__main__":
    raise SystemExit(main())
