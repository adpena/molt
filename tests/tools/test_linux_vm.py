"""tools/linux_vm.py: VM definitions, drift, pinned-file release and apply.

The fake host emulates the two external tools the module reads. Its lsof
applies lsof's own selection rules (selections OR unless ``-a``; ``+L`` with
no number adds link counts, ``+L1`` selects unlinked files) and lists files of
processes that are not VMs, so a parser that trusts the argv to filter, or
counts the wrong descriptors, reports the wrong numbers. Its guest drops only
the inodes no guest process holds, as ``drop_caches`` does, so a release that
claims to free open files fails. Lima's own validator is the oracle for the
rendered YAML when ``limactl`` is installed.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import shutil
import time

import pytest

from tools import linux_vm

ROOT = Path(__file__).resolve().parents[2]
VZ_BINARY = (
    "/System/Library/Frameworks/Virtualization.framework/Versions/A/XPCServices/"
    "com.apple.Virtualization.VirtualMachine.xpc/Contents/MacOS/"
    "com.apple.Virtualization.VirtualMachine"
)
DEAD_PID = 2**31  # beyond every POSIX pid, so never alive


# --------------------------------------------------------------------------
# Fake host
# --------------------------------------------------------------------------


@dataclass
class HostFile:
    name: str
    size: int
    links: int
    fd: str
    kind: str = "REG"


@dataclass
class FakeVm:
    name: str
    directory: Path
    config: dict[str, object]
    status: str = "Running"
    vm_type: str = "vz"
    arch: str = "aarch64"
    cpus: int = 6
    memory: int = 12 << 30
    disk: int = 225 << 30
    vz_pid: int | None = 9001
    # Host files the guest caches (inodes it has not evicted): dropped by
    # drop_caches. `held_open` are files a guest process has open: kept.
    cached: list[HostFile] = field(default_factory=list)
    held_open: list[HostFile] = field(default_factory=list)
    guest_uid: int = 501
    processes: list[tuple[int, int, int, str]] = field(default_factory=list)
    probe_shell: int = 4000

    def json(self) -> str:
        return json.dumps(
            {
                "name": self.name,
                "status": self.status,
                "dir": str(self.directory),
                "vmType": self.vm_type,
                "arch": self.arch,
                "cpus": self.cpus,
                "memory": self.memory,
                "disk": self.disk,
                "config": self.config,
            }
        )

    def vz_files(self) -> list[HostFile]:
        files = [
            HostFile("/", 704, 22, "cwd", "DIR"),
            HostFile(VZ_BINARY, 8178240, 1, "txt"),
            # The VM process's own deleted mapping, not a share pin.
            HostFile("/Library/Preferences/Logging/.plist-cache.X", 72744, 0, "txt"),
            HostFile(str(self.directory / "disk"), self.disk, 1, "3"),
            HostFile(str(self.directory / "cidata.iso"), 4096, 1, "4"),
        ]
        return files + self.cached + self.held_open


@dataclass
class OtherProcess:
    pid: int
    command: str
    files: list[HostFile]


class FakeHost:
    def __init__(
        self,
        vms: Sequence[FakeVm] = (),
        *,
        others: Sequence[OtherProcess] = (),
        lima: bool = True,
        validate_rc: int = 0,
        drop_rc: int = 0,
        on_start: Callable[[FakeVm], None] | None = None,
    ) -> None:
        self.vms = {vm.name: vm for vm in vms}
        self.others = list(others)
        self.lima = lima
        self.validate_rc = validate_rc
        self.drop_rc = drop_rc
        self.on_start = on_start
        self.calls: list[tuple[str, ...]] = []
        self.validated: list[str] = []
        # Lima reads an instance's config from its lima.yaml on every list.
        self.configs_by_text: dict[str, dict[str, object]] = {}
        self.start_rc = 0

    def _listed(self, vm: FakeVm) -> str:
        text = (vm.directory / "lima.yaml").read_text(encoding="utf-8")
        if text in self.configs_by_text:
            vm.config = self.configs_by_text[text]
        return vm.json()

    def which(self, name: str) -> str | None:
        if name == "limactl" and not self.lima:
            return None
        return f"/fake/bin/{name}"

    def run(self, argv: Sequence[str], *, timeout: float) -> linux_vm.Completed:
        argv = tuple(argv)
        self.calls.append(argv)
        assert timeout > 0
        match argv:
            case ("limactl", "list", "--json", *names):
                rows = [
                    self._listed(vm)
                    for vm in self.vms.values()
                    if not names or vm.name in names
                ]
                return linux_vm.Completed(0, "".join(row + "\n" for row in rows), "")
            case ("limactl", "validate", path):
                self.validated.append(Path(path).read_text(encoding="utf-8"))
                return linux_vm.Completed(
                    self.validate_rc, "", "" if self.validate_rc == 0 else "bad field"
                )
            case (
                "limactl",
                "shell",
                "--workdir",
                "/",
                name,
                "sudo",
                "-n",
                "sh",
                "-c",
                script,
            ):
                assert script == linux_vm.DROP_GUEST_CACHES, script
                vm = self.vms[name]
                assert vm.status == "Running"
                if self.drop_rc == 0:
                    vm.cached = []
                return linux_vm.Completed(
                    self.drop_rc,
                    "",
                    "" if self.drop_rc == 0 else "sudo: a password is required",
                )
            case ("limactl", "shell", "--workdir", "/", name, "sh", "-c", script):
                assert script == linux_vm.IDLE_PROBE, script
                vm = self.vms[name]
                lines = [str(vm.guest_uid), str(vm.probe_shell)]
                lines += [
                    f"{pid} {ppid} {uid} {comm}"
                    for pid, ppid, uid, comm in vm.processes
                ]
                return linux_vm.Completed(0, "\n".join(lines) + "\n", "")
            case ("limactl", "stop", name):
                self.vms[name].status = "Stopped"
                return linux_vm.Completed(0, "", "")
            case ("limactl", "start", "--tty=false", name):
                vm = self.vms[name]
                if self.start_rc:
                    return linux_vm.Completed(self.start_rc, "", "vz: boot failed")
                vm.status = "Running"
                (vm.directory / "ha.pid").write_text("1\n", encoding="utf-8")
                os.utime(vm.directory / "ha.pid", (time.time() + 5, time.time() + 5))
                if self.on_start is not None:
                    self.on_start(vm)
                return linux_vm.Completed(0, "", "")
            case ("lsof", *options):
                return self._lsof(options)
        raise AssertionError(f"unexpected command {argv}")

    def _lsof(self, options: Sequence[str]) -> linux_vm.Completed:
        conjunction = False
        commands: list[str] = []
        pids: list[int] = []
        link_limit: int | None = None
        links_listed = False
        fields = ""
        items = iter(options)
        for option in items:
            if option == "-a":
                conjunction = True
            elif option == "-c":
                commands.append(next(items))
            elif option == "-p":
                pids.extend(int(p) for p in next(items).split(","))
            elif option == "+L":
                links_listed = True
            elif option.startswith("+L"):
                links_listed = True
                link_limit = int(option[2:])
            elif option == "-F":
                fields = next(items)
            elif option in ("-n", "-P"):
                continue
            else:
                raise AssertionError(f"unexpected lsof option {option}")
        processes = [
            (vm.vz_pid, linux_vm.VZ_COMMAND, vm.vz_files())
            for vm in self.vms.values()
            if vm.status == "Running" and vm.vz_pid is not None
        ] + [(other.pid, other.command, other.files) for other in self.others]
        out: list[str] = []
        for pid, command, files in sorted(processes):
            process_tests = []
            if commands:
                process_tests.append(any(command.startswith(c) for c in commands))
            if pids:
                process_tests.append(pid in pids)
            listed = []
            for item in files:
                tests = list(process_tests)
                if link_limit is not None:
                    tests.append(item.links < link_limit)
                if not tests:
                    selected = True
                elif conjunction:
                    selected = all(tests)
                else:
                    selected = any(tests)
                if selected:
                    listed.append(item)
            if not listed:
                continue
            out.append(f"p{pid}")
            if "c" in fields:
                out.append(f"c{command}")
            for item in listed:
                out.append(f"f{item.fd}")
                if "t" in fields:
                    out.append(f"t{item.kind}")
                if "s" in fields:
                    out.append(f"s{item.size}")
                if "k" in fields and links_listed:
                    out.append(f"k{item.links}")
                if "n" in fields:
                    out.append(f"n{item.name}")
        return linux_vm.Completed(
            0 if out else 1, "".join(line + "\n" for line in out), ""
        )


# --------------------------------------------------------------------------
# Fixtures
# --------------------------------------------------------------------------


def lima_filled(rendered: dict[str, object]) -> dict[str, object]:
    """What `limactl list --json` shows for an instance made from `rendered`.

    Written from `limactl validate --fill` output (Lima 2.2.1): Lima adds
    mountPoint and the sshfs/9p blocks to each mount and drops `ignore:
    false`; every other rendered key survives as given.
    """
    config = json.loads(json.dumps(rendered))
    for mount in config.get("mounts", []):
        mount["mountPoint"] = mount["location"]
        mount["sshfs"] = {"cache": True, "followSymlinks": False, "sftpDriver": ""}
        mount["9p"] = {"securityModel": "none", "cache": "fscache"}
    for rule in config.get("portForwards", []):
        if rule.get("ignore") is False:
            del rule["ignore"]
    config["containerd"]["archives"] = [
        {"location": "https://example.invalid/nerdctl.tgz"}
    ]
    config["ssh"] = {"localPort": 0}
    return config


@pytest.fixture
def definitions() -> linux_vm.Definitions:
    return linux_vm.load_definitions()


def make_instance_dir(
    base: Path, name: str, *, started_after_config: bool = True
) -> Path:
    directory = base / name
    directory.mkdir(parents=True)
    (directory / "lima.yaml").write_text(
        '# hand-made\nmounts:\n- location: "~"\n', encoding="utf-8"
    )
    (directory / "ha.pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
    now = time.time()
    config_time, start_time = (
        (now - 100, now - 50) if started_after_config else (now - 50, now - 100)
    )
    os.utime(directory / "lima.yaml", (config_time, config_time))
    os.utime(directory / "ha.pid", (start_time, start_time))
    return directory


def matching_vm(
    definitions: linux_vm.Definitions,
    name: str,
    base: Path,
    custody: Path,
    **overrides: object,
) -> FakeVm:
    vm = definitions.vms[name]
    rendered = linux_vm.lima_config(
        vm, custody=custody, minimum_lima_version=definitions.minimum_lima_version
    )
    fake = FakeVm(
        name=name,
        directory=make_instance_dir(base, name),
        config=lima_filled(rendered),
        cpus=vm.cpus,
        memory=vm.memory.bytes,
        disk=vm.disk.bytes,
    )
    for key, value in overrides.items():
        setattr(fake, key, value)
    return fake


def instance_of(fake: FakeVm) -> linux_vm.Instance:
    return linux_vm.list_instances(FakeHost([fake]))[0]


# --------------------------------------------------------------------------
# Definitions
# --------------------------------------------------------------------------


def test_checked_in_vms_mount_only_the_custody_root_read_only_and_forward_nothing(
    definitions: linux_vm.Definitions,
) -> None:
    assert set(definitions.vms) == {"molt-linux", "molt-x86"}
    for vm in definitions.vms.values():
        assert vm.mounts == (
            linux_vm.MountDefinition(linux_vm.MountSource.CUSTODY_ROOT, writable=False),
        )
        assert vm.port_forwards == ()
        assert linux_vm.port_rules(vm)[-1] == linux_vm.IGNORE_EVERY_GUEST_PORT
    assert definitions.vms["molt-x86"].rosetta
    assert not definitions.vms["molt-linux"].rosetta
    text = linux_vm.DEFINITIONS_PATH.read_text(encoding="utf-8")
    assert "/Users/" not in text and "/home/" not in text, "no personal host path"


def _linked_worktree(main: Path, checkout: Path, name: str) -> None:
    admin = main / ".git" / "worktrees" / name
    admin.mkdir(parents=True)
    (admin / "HEAD").write_text("ref: refs/heads/lane\n", encoding="utf-8")
    (admin / "commondir").write_text("../..\n", encoding="utf-8")
    checkout.mkdir(parents=True)
    (checkout / ".git").write_text(f"gitdir: {admin}\n", encoding="utf-8")


def test_every_checkout_of_a_family_mounts_the_same_custody_root(
    tmp_path: Path,
) -> None:
    root = tmp_path / "Molt"
    main = root / "molt-src"
    (main / ".git").mkdir(parents=True)
    (main / ".git" / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
    lane = root / "worktrees" / "lane"
    _linked_worktree(main, lane, "lane")
    # A worktree outside the family layout, named like a main checkout.
    foreign = tmp_path / "codex" / "task" / "molt-src"
    _linked_worktree(main, foreign, "task")
    expected = root.resolve()
    assert linux_vm.family_custody_root(main) == expected
    assert linux_vm.family_custody_root(lane) == expected
    assert linux_vm.family_custody_root(foreign) == expected


_VALID = """\
schema = 1
minimum_lima_version = "2.0.0"
idle_guest_processes = ["systemd"]
maintenance_guest_processes = ["dpkg"]

[vm.a]
purpose = "test"
vm_type = "vz"
arch = "aarch64"
cpus = 2
memory = "4GiB"
disk = "20GiB"
rosetta = false
containerd_user = false
mount_type = "virtiofs"
params = {}
mounts = [{ source = "custody-root", writable = false }]
port_forwards = []

[vm.a.image]
location = "https://example.invalid/a.img"
digest = "sha256:%s"
""" % ("0" * 64)


@pytest.mark.parametrize(
    ("old", "new", "message"),
    [
        ("cpus = 2", "cpus = 2\nextra = 1", "unknown ['extra']"),
        ('memory = "4GiB"', 'memory = "4GB"', "must look like 12GiB"),
        ('source = "custody-root"', 'source = "home"', "must be one of: custody-root"),
        (
            "writable = false }]",
            'writable = false }, { source = "custody-root", writable = true }]',
            "names one source twice",
        ),
        (
            'mount_type = "virtiofs"',
            'mount_type = "reverse-sshfs"',
            "must be one of: virtiofs",
        ),
        (
            "port_forwards = []",
            'port_forwards = [{ guest_port = 0, host_port = 1, proto = "tcp" }]',
            "port from 1 to 65535",
        ),
        (
            "port_forwards = []",
            'port_forwards = [{ guest_port = 80, host_port = 8080, proto = "any" }]',
            "must be one of: tcp, udp",
        ),
        ('digest = "sha256:', 'digest = "md5:', "sha256:<64 hex>"),
        ("schema = 1", "schema = 2", "schema must be 1"),
    ],
)
def test_definition_schema_rejects_what_it_does_not_define(
    tmp_path: Path, old: str, new: str, message: str
) -> None:
    path = tmp_path / "vms.toml"
    assert old in _VALID
    path.write_text(_VALID.replace(old, new, 1), encoding="utf-8")
    with pytest.raises(
        linux_vm.DefinitionError, match=message.replace("[", r"\[").replace("]", r"\]")
    ):
        linux_vm.load_definitions(path)
    path.write_text(_VALID, encoding="utf-8")
    assert linux_vm.load_definitions(path).vms["a"].memory.bytes == 4 << 30


# --------------------------------------------------------------------------
# Rendering
# --------------------------------------------------------------------------


def test_rendered_rules_put_declared_forwards_before_the_rule_that_ignores_the_rest(
    tmp_path: Path,
) -> None:
    path = tmp_path / "vms.toml"
    path.write_text(
        _VALID.replace(
            "port_forwards = []",
            'port_forwards = [{ guest_port = 8080, host_port = 18080, proto = "tcp" }]',
        ),
        encoding="utf-8",
    )
    vm = linux_vm.load_definitions(path).vms["a"]
    config = linux_vm.lima_config(vm, custody=tmp_path, minimum_lima_version="2.0.0")
    assert config["portForwards"] == [
        {
            "guestIP": "127.0.0.1",
            "guestIPMustBeZero": False,
            "guestPortRange": [8080, 8080],
            "hostIP": "127.0.0.1",
            "hostPortRange": [18080, 18080],
            "proto": "tcp",
        },
        {
            "guestIP": "0.0.0.0",
            "guestIPMustBeZero": False,
            "guestPortRange": [1, 65535],
            "hostIP": "127.0.0.1",
            "hostPortRange": [1, 65535],
            "proto": "any",
            "ignore": True,
        },
    ]
    assert config["mounts"] == [{"location": str(tmp_path), "writable": False}]
    text = linux_vm.render(vm, custody=tmp_path, minimum_lima_version="2.0.0")
    assert '  - guestIP: "127.0.0.1"\n' in text
    assert "    ignore: true\n" in text
    assert text.index("18080") < text.index("ignore: true")


@pytest.mark.skipif(shutil.which("limactl") is None, reason="Lima is not installed")
@pytest.mark.parametrize("name", ["molt-linux", "molt-x86"])
def test_lima_parses_the_rendered_config_as_the_definition_says(
    definitions: linux_vm.Definitions, tmp_path: Path, name: str
) -> None:
    """Lima's own parser (`limactl validate --fill`) reads the rendered YAML."""
    vm = definitions.vms[name]
    custody = tmp_path / "custody root"
    path = tmp_path / f"{name}.yaml"
    path.write_text(
        linux_vm.render(
            vm, custody=custody, minimum_lima_version=definitions.minimum_lima_version
        ),
        encoding="utf-8",
    )
    filled = linux_vm.LocalHost(os.environ).run(
        ["limactl", "validate", "--fill", str(path)], timeout=60.0
    )
    assert filled.returncode == 0, filled.stderr
    text = filled.stdout
    assert f"cpus: {vm.cpus}\n" in text
    assert f"memory: {vm.memory.text}\n" in text
    assert "mountType: virtiofs\n" in text
    assert (
        f"- location: {custody}\n  mountPoint: {custody}\n  writable: false\n" in text
    )
    assert "  proto: any\n  ignore: true\n" in text
    assert text.count("ignore: true") == 1
    assert ("binfmt: true" in text) == vm.rosetta


# --------------------------------------------------------------------------
# Drift
# --------------------------------------------------------------------------


def _set(path: str, value: object) -> Callable[[dict[str, object]], None]:
    def edit(config: dict[str, object]) -> None:
        *parents, key = path.split(".")
        target: object = config
        for part in parents:
            target = (
                target[int(part)]
                if isinstance(target, list)
                else target.setdefault(part, {})
            )
        if value is _DELETE:
            del target[key]  # type: ignore[index]
        else:
            target[key] = value  # type: ignore[index]

    return edit


_DELETE = object()


@pytest.mark.parametrize(
    ("name", "edit", "fields"),
    [
        ("molt-linux", None, set()),
        ("molt-x86", None, set()),
        # An undeclared mountType is virtiofs under vz.
        ("molt-linux", _set("mountType", _DELETE), set()),
        ("molt-linux", _set("mountType", "reverse-sshfs"), {"mount_type"}),
        ("molt-linux", _set("mounts.0.writable", True), {"mounts"}),
        ("molt-linux", _set("mounts.0.location", "/"), {"mounts"}),
        ("molt-linux", _set("mounts.0.mountPoint", "/mnt/molt"), {"mounts"}),
        ("molt-linux", _set("portForwards", None), {"port_forwards"}),
        (
            "molt-linux",
            _set("portForwards.0.guestIPMustBeZero", True),
            {"port_forwards"},
        ),
        ("molt-linux", _set("portForwards.0.ignore", _DELETE), {"port_forwards"}),
        ("molt-linux", _set("containerd.user", False), {"containerd"}),
        ("molt-linux", _set("param", _DELETE), {"params"}),
        ("molt-x86", _set("vmOpts", _DELETE), {"rosetta"}),
        ("molt-linux", _set("images.0.digest", "sha256:" + "f" * 64), {"images"}),
    ],
)
def test_compare_names_exactly_the_fields_that_differ(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    name: str,
    edit: Callable[[dict[str, object]], None] | None,
    fields: set[str],
) -> None:
    custody = tmp_path / "custody"
    fake = matching_vm(definitions, name, tmp_path / "lima", custody)
    if edit is not None:
        edit(fake.config)
    drift = linux_vm.compare(definitions.vms[name], instance_of(fake), custody=custody)
    assert {str(item.field) for item in drift} == fields


def test_compare_reads_an_extra_forward_rule_ahead_of_the_ignore_rule_as_drift(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    custody = tmp_path / "custody"
    fake = matching_vm(definitions, "molt-linux", tmp_path / "lima", custody)
    fake.config["portForwards"].insert(  # type: ignore[union-attr]
        0,
        {
            "guestIPMustBeZero": False,
            "guestIP": "127.0.0.1",
            "guestPortRange": [4226, 4226],
            "hostIP": "127.0.0.1",
            "hostPortRange": [4226, 4226],
            "proto": "tcp",
        },
    )
    drift = linux_vm.compare(
        definitions.vms["molt-linux"], instance_of(fake), custody=custody
    )
    assert [item.field for item in drift] == [linux_vm.DriftField.PORT_FORWARDS]
    assert "4226" in drift[0].effective


def test_compare_reads_resources_from_the_instance(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    custody = tmp_path / "custody"
    fake = matching_vm(
        definitions,
        "molt-x86",
        tmp_path / "lima",
        custody,
        cpus=2,
        memory=7 << 30,
        disk=100 << 30,
    )
    drift = linux_vm.compare(
        definitions.vms["molt-x86"], instance_of(fake), custody=custody
    )
    assert {(d.field, d.declared, d.effective) for d in drift} == {
        (linux_vm.DriftField.CPUS, "4", "2"),
        (linux_vm.DriftField.MEMORY, "8GiB", "7GiB"),
        (linux_vm.DriftField.DISK, "40GiB", "100GiB"),
    }


@pytest.mark.parametrize(
    ("status", "started_after_config", "pending"),
    [("Running", True, False), ("Running", False, True), ("Stopped", False, False)],
)
def test_a_config_edited_after_start_is_drift_until_the_vm_restarts(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    status: str,
    started_after_config: bool,
    pending: bool,
) -> None:
    custody = tmp_path / "custody"
    fake = matching_vm(
        definitions, "molt-linux", tmp_path / "lima", custody, status=status
    )
    shutil.rmtree(fake.directory)
    fake.directory = make_instance_dir(
        tmp_path / "lima", "molt-linux", started_after_config=started_after_config
    )
    drift = linux_vm.compare(
        definitions.vms["molt-linux"], instance_of(fake), custody=custody
    )
    assert [item.field for item in drift] == (
        [linux_vm.DriftField.RESTART_PENDING] if pending else []
    )


# --------------------------------------------------------------------------
# Held files
# --------------------------------------------------------------------------


def two_vms(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> tuple[FakeVm, FakeVm, Path]:
    custody = tmp_path / "custody"
    custody.mkdir()
    linux = matching_vm(
        definitions, "molt-linux", tmp_path / "lima", custody, vz_pid=9200
    )
    x86 = matching_vm(definitions, "molt-x86", tmp_path / "lima", custody, vz_pid=9100)
    # molt-x86 mounts the custody root; molt-linux has drifted to its parent.
    linux.config["mounts"][0]["location"] = str(tmp_path)  # type: ignore[index]
    linux.config["mounts"][0]["mountPoint"] = str(tmp_path)  # type: ignore[index]
    linux.cached = [
        HostFile(str(custody / "worktrees/old/a.py"), 100, 0, "10"),
        HostFile("/3f1c0ffee", 1000, 0, "11"),  # lsof's partial name of a deleted file
        HostFile(str(custody / "molt-src/README.md"), 50, 1, "12"),
        HostFile(str(tmp_path / "elsewhere.txt"), 50, 1, "13"),
        HostFile(str(custody / "worktrees"), 0, 0, "14", "DIR"),
    ]
    linux.held_open = [HostFile(str(custody / "tmp/log"), 7, 0, "15")]
    x86.cached = [HostFile(str(custody / "molt-src/x"), 5, 1, "8")]
    return linux, x86, custody


def test_held_files_maps_each_vm_process_by_its_disk_and_counts_only_share_pins(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    linux, x86, _custody = two_vms(definitions, tmp_path)
    foreign_vm = OtherProcess(
        9050,
        linux_vm.VZ_COMMAND,
        [
            HostFile("/Users/x/Library/Containers/docker/disk", 1, 1, "3"),
            HostFile("/gone", 999, 0, "9"),
        ],
    )
    finder = OtherProcess(
        400, "Finder", [HostFile("/Users/x/.Trash/big", 5000, 0, "20")]
    )
    host = FakeHost([linux, x86], others=[foreign_vm, finder])
    held = linux_vm.held_files(host, linux_vm.list_instances(host))
    assert held == {
        # Deleted regular files on numbered descriptors: the share's pins.
        # The deleted txt mapping and the deleted directory are not counted.
        # The VM's own disk lies under its drifted mount and is not counted.
        "molt-linux": linux_vm.Held(
            pid=9200, deleted_files=3, deleted_bytes=1107, open_under_mounts=2
        ),
        "molt-x86": linux_vm.Held(
            pid=9100, deleted_files=0, deleted_bytes=0, open_under_mounts=1
        ),
    }
    assert [call[0] for call in host.calls] == ["limactl", "lsof"]


# --------------------------------------------------------------------------
# Release
# --------------------------------------------------------------------------


def test_release_frees_cached_pins_keeps_open_files_and_never_stops_a_vm(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    linux, x86, _custody = two_vms(definitions, tmp_path)
    stopped = matching_vm(
        definitions, "molt-x86", tmp_path / "other", tmp_path, status="Stopped"
    )
    stopped.name = "stopped-vm"
    host = FakeHost([linux, x86, stopped])
    results = linux_vm.release(host, linux_vm.list_instances(host))
    by_name = {result.name: result for result in results}
    assert set(by_name) == {"molt-linux", "molt-x86"}
    assert by_name["molt-linux"].before.deleted_files == 3
    assert by_name["molt-linux"].after == linux_vm.Held(
        pid=9200, deleted_files=1, deleted_bytes=7, open_under_mounts=0
    )
    assert by_name["molt-x86"].after.open_under_mounts == 0
    assert all(result.error is None for result in results)
    drops = [call for call in host.calls if call[:2] == ("limactl", "shell")]
    assert sorted(call[4] for call in drops) == ["molt-linux", "molt-x86"]
    assert all(
        call[5:] == ("sudo", "-n", "sh", "-c", "echo 2 > /proc/sys/vm/drop_caches")
        for call in drops
    )
    assert not [call for call in host.calls if call[1:2] in (("stop",), ("start",))]


def test_release_command_reports_a_failed_drop(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    linux, x86, _custody = two_vms(definitions, tmp_path)
    host = FakeHost([linux, x86], drop_rc=1)
    assert linux_vm.release_command(host=host, names=["molt-linux"], as_json=True) == 1
    report = json.loads(capsys.readouterr().out)
    assert report["schema"] == linux_vm.RELEASE_SCHEMA
    [row] = report["vms"]
    assert row["name"] == "molt-linux"
    assert "password is required" in row["error"]
    assert row["before"]["deleted_files"] == row["after"]["deleted_files"] == 3
    assert linux_vm.release_command(host=host, names=["nope"], as_json=False) == 2


# --------------------------------------------------------------------------
# The retirement hook
# --------------------------------------------------------------------------


def test_the_hook_spawns_nothing_without_a_live_hostagent(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    linux, x86, custody = two_vms(definitions, tmp_path)
    host = FakeHost([linux, x86])
    path = custody / "molt-src"
    assert (
        linux_vm.release_for_paths(
            [path], environment={"LIMA_HOME": str(tmp_path / "absent")}, host=host
        )
        == ()
    )
    # Instance directories whose hostagent pid is dead do not count.
    for directory in (linux.directory, x86.directory):
        (directory / "ha.pid").write_text(f"{DEAD_PID}\n", encoding="utf-8")
    assert (
        linux_vm.release_for_paths(
            [path], environment={"LIMA_HOME": str(tmp_path / "lima")}, host=host
        )
        == ()
    )
    assert host.calls == []


def test_the_hook_does_not_inspect_vms_that_do_not_mount_the_path(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    linux, x86, _custody = two_vms(definitions, tmp_path)
    host = FakeHost([linux, x86])
    outside = tmp_path.parent / "not-mounted"
    assert (
        linux_vm.release_for_paths(
            [outside], environment={"LIMA_HOME": str(tmp_path / "lima")}, host=host
        )
        == ()
    )
    assert host.calls == [("limactl", "list", "--json")]


def test_the_hook_skips_the_drop_when_no_covering_vm_holds_files_there(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    linux, x86, custody = two_vms(definitions, tmp_path)
    linux.vz_pid = None  # molt-linux's VM process is gone
    host = FakeHost([linux, x86])
    # molt-x86 caches custody/molt-src/x, but nothing under this path.
    assert (
        linux_vm.release_for_paths(
            [custody / "worktrees" / "fresh"],
            environment={"LIMA_HOME": str(tmp_path / "lima")},
            host=host,
        )
        == ()
    )
    assert [call[0] for call in host.calls] == ["limactl", "lsof"]
    assert x86.cached


def test_the_hook_releases_only_covering_vms_that_hold_files_under_the_path(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    linux, x86, _custody = two_vms(definitions, tmp_path)
    host = FakeHost([linux, x86])
    # Only molt-linux's drifted mount (the custody root's parent) covers it.
    doomed = tmp_path / "elsewhere.txt"
    results = linux_vm.release_for_paths(
        [doomed], environment={"LIMA_HOME": str(tmp_path / "lima")}, host=host
    )
    assert [r.name for r in results] == ["molt-linux"]
    [result] = results
    assert (result.before.open_under_paths, result.after.open_under_paths) == (1, 0)
    assert (result.before.deleted_files, result.after.deleted_files) == (3, 1)
    assert x86.cached, "a VM that does not mount the path keeps its caches"
    # One snapshot decides and serves as the "before" measurement.
    assert [call[:2] for call in host.calls] == [
        ("limactl", "list"),
        ("lsof", "-n"),
        ("limactl", "shell"),
        ("lsof", "-n"),
    ]


def test_the_hook_releases_a_vm_that_caches_files_under_the_path_and_pins_nothing(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    linux, x86, custody = two_vms(definitions, tmp_path)
    linux.vz_pid = None
    host = FakeHost([linux, x86])
    # molt-x86 caches custody/molt-src/x and holds no deleted file yet.
    results = linux_vm.release_for_paths(
        [custody / "molt-src"],
        environment={"LIMA_HOME": str(tmp_path / "lima")},
        host=host,
    )
    assert [
        (r.name, r.before.open_under_paths, r.after.open_under_paths) for r in results
    ] == [("molt-x86", 1, 0)]
    assert not x86.cached


def test_the_hook_also_frees_deleted_files_a_covering_vm_already_pins(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    linux, x86, custody = two_vms(definitions, tmp_path)
    linux.vz_pid = None
    x86.cached.append(HostFile("/9a8b", 4096, 0, "9"))  # e.g. a pack `git gc` deleted
    host = FakeHost([linux, x86])
    results = linux_vm.release_for_paths(
        [custody / "worktrees" / "fresh"],
        environment={"LIMA_HOME": str(tmp_path / "lima")},
        host=host,
    )
    assert [
        (r.name, r.before.deleted_bytes, r.after.deleted_bytes) for r in results
    ] == [("molt-x86", 4096, 0)]


# --------------------------------------------------------------------------
# Apply
# --------------------------------------------------------------------------

SESSION = [
    (1, 0, 0, "systemd"),
    (300, 1, 0, "lima-guestagent"),
    (310, 1, 0, "unattended-upgr"),  # unattended-upgrade-shutdown, resident
    (900, 1, 501, "systemd"),
    (901, 900, 501, "(sd-pam)"),
    (950, 900, 501, "containerd-shim"),  # containerd-shim-runc-v2, truncated
    (3900, 1, 0, "sshd-session"),
    (3901, 3900, 501, "sshd-session"),
    (3950, 3901, 501, "bash"),  # this probe's login shell
    (4000, 3950, 501, "sh"),  # the probe shell
    (4001, 4000, 501, "ps"),
]


def test_busy_guest_processes_exempts_this_session_and_guest_daemons(
    definitions: linux_vm.Definitions,
) -> None:
    def probe(processes: list[tuple[int, int, int, str]]) -> str:
        return "501\n4000\n" + "".join(
            f"{p} {pp} {u} {c}\n" for p, pp, u, c in processes
        )

    assert linux_vm.busy_guest_processes(probe(SESSION), definitions) == ()
    busy = SESSION + [
        (5000, 1, 501, "sleep"),  # a detached loop of another session
        (5100, 1, 0, "dpkg"),  # a package upgrade as root
        (5200, 1, 0, "cron"),  # root daemons are not the guest user's work
        (5300, 5299, 501, "cargo"),
        (5400, 1, 501, "python3"),  # any other process of the guest user
    ]
    assert {p.pid for p in linux_vm.busy_guest_processes(probe(busy), definitions)} == {
        5000,
        5100,
        5300,
        5400,
    }
    with pytest.raises(linux_vm.LimaError, match="own shell"):
        linux_vm.busy_guest_processes("501\n4242\n1 0 0 systemd\n", definitions)


def apply_setup(
    definitions: linux_vm.Definitions, tmp_path: Path, **host_options: object
) -> tuple[FakeHost, FakeVm, Path, str]:
    custody = tmp_path / "custody"
    fake = matching_vm(definitions, "molt-linux", tmp_path / "lima", custody)
    fake.config["mounts"][0]["location"] = str(tmp_path)  # type: ignore[index]
    fake.config["portForwards"] = None
    fake.processes = list(SESSION)
    rendered = linux_vm.lima_config(
        definitions.vms["molt-linux"],
        custody=custody,
        minimum_lima_version=definitions.minimum_lima_version,
    )

    text = linux_vm.render(
        definitions.vms["molt-linux"],
        custody=custody,
        minimum_lima_version=definitions.minimum_lima_version,
    )
    host = FakeHost([fake], **host_options)
    host.configs_by_text[text] = lima_filled(rendered)
    original = (fake.directory / "lima.yaml").read_text(encoding="utf-8")
    return host, fake, custody, original


def run_apply(definitions, host, custody, *, confirm: bool = True) -> int:
    return linux_vm.apply(
        "molt-linux",
        definitions=definitions,
        host=host,
        custody=custody,
        confirm_restart=confirm,
    )


def test_apply_refuses_without_confirmation_and_runs_nothing(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, custody, original = apply_setup(definitions, tmp_path)
    assert run_apply(definitions, host, custody, confirm=False) == 1
    assert host.calls == []
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original


def test_apply_refuses_a_busy_guest_and_leaves_the_vm_running(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    host, fake, custody, original = apply_setup(definitions, tmp_path)
    fake.processes.append((6000, 3950, 501, "cargo"))
    assert run_apply(definitions, host, custody) == 1
    assert "busy: cargo (pid 6000, uid 501)" in capsys.readouterr().out
    assert fake.status == "Running"
    assert not [call for call in host.calls if call[1] in ("stop", "start")]
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original
    assert not (fake.directory / linux_vm.NEXT_CONFIG).exists()


def test_apply_refuses_a_config_lima_rejects_before_touching_the_guest(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, custody, original = apply_setup(definitions, tmp_path, validate_rc=1)
    assert run_apply(definitions, host, custody) == 1
    assert [call[1] for call in host.calls] == ["list", "validate"]
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original
    assert not (fake.directory / linux_vm.NEXT_CONFIG).exists()


def test_apply_refuses_a_change_only_a_new_vm_can_make(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, custody, _original = apply_setup(definitions, tmp_path)
    fake.vm_type = "qemu"
    assert run_apply(definitions, host, custody) == 1
    assert [call[1] for call in host.calls] == ["list"]


def test_apply_installs_the_definition_into_an_idle_vm_and_restarts_it(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, custody, original = apply_setup(definitions, tmp_path)
    assert run_apply(definitions, host, custody) == 0
    verbs = [call[1] for call in host.calls]
    assert verbs == ["list", "validate", "shell", "stop", "start", "list"]
    assert host.validated == [
        (fake.directory / "lima.yaml").read_text(encoding="utf-8")
    ]
    assert host.validated[0] in host.configs_by_text
    assert (fake.directory / linux_vm.PREVIOUS_CONFIG).read_text(
        encoding="utf-8"
    ) == original
    assert not (fake.directory / linux_vm.NEXT_CONFIG).exists()
    assert fake.status == "Running"
    assert (
        linux_vm.compare(
            definitions.vms["molt-linux"], instance_of(fake), custody=custody
        )
        == ()
    )


def test_apply_to_a_stopped_vm_installs_the_config_without_starting_it(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, custody, original = apply_setup(definitions, tmp_path)
    fake.status = "Stopped"
    assert run_apply(definitions, host, custody) == 0
    assert [call[1] for call in host.calls] == ["list", "validate", "list"]
    assert fake.status == "Stopped"
    assert (fake.directory / linux_vm.PREVIOUS_CONFIG).read_text(
        encoding="utf-8"
    ) == original


def test_apply_reports_a_failed_start_and_keeps_the_previous_config(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    host, fake, custody, original = apply_setup(definitions, tmp_path)
    host.start_rc = 1
    assert run_apply(definitions, host, custody) == 1
    assert "boot failed" in capsys.readouterr().out
    assert (fake.directory / linux_vm.PREVIOUS_CONFIG).read_text(
        encoding="utf-8"
    ) == original


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def test_check_exits_one_on_drift_and_zero_when_every_vm_matches(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    linux, x86, custody = two_vms(definitions, tmp_path)
    host = FakeHost([linux, x86])
    assert linux_vm.main(["check", "--json"], host=host, custody=custody) == 1
    report = json.loads(capsys.readouterr().out)
    assert report["schema"] == linux_vm.CHECK_SCHEMA
    rows = {row["name"]: row for row in report["vms"]}
    assert [d["field"] for d in rows["molt-linux"]["drift"]] == ["mounts"]
    assert rows["molt-x86"]["drift"] == []
    assert rows["molt-linux"]["held"]["deleted_files"] == 3
    assert not [call for call in host.calls if call[1:2] == ("shell",)], (
        "check runs nothing in a guest"
    )
    linux.config["mounts"][0]["location"] = str(custody)  # type: ignore[index]
    linux.config["mounts"][0]["mountPoint"] = str(custody)  # type: ignore[index]
    assert linux_vm.main(["check"], host=host, custody=custody) == 0


def test_check_without_lima_has_nothing_to_check(tmp_path: Path) -> None:
    host = FakeHost(lima=False)
    assert linux_vm.main(["check"], host=host, custody=tmp_path) == 0
    assert host.calls == []
