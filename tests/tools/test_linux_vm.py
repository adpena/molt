"""tools/linux_vm.py: VM definitions, drift, push, pinned-file release, apply.

The fake host emulates the external tools the module reads. Its lsof applies
lsof's own selection rules (selections OR unless ``-a``; ``+L`` with no number
adds link counts, ``+L1`` selects unlinked files) and lists files of
processes that are not VMs, so a parser that trusts the argv to filter, or
counts the wrong descriptors, reports the wrong numbers.

Its guest and virtiofs server follow what the real VMs showed
(docs/design/linux_vm_apparatus.md, "Mechanism"): a lookup opens a host
descriptor; ``drop_caches`` makes the guest forget every inode no guest
process uses; the server closes forgotten descriptors only when its 64th
pending forget arrives. So a release that only drops caches frees nothing
here, as it freed nothing on the real VMs. ``SERVER_FORGET_BATCH`` is the
measured number, kept apart from the module's constant. The opt-in test at
the end checks the release against a real guest.

Lima's own validator is the oracle for the rendered YAML when ``limactl`` is
installed; real Git is the oracle for ``push``.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import shlex
import shutil
import sys
import time
import uuid

import pytest

from molt.process_environment import ProcessEnvironment
from molt.process_model import pid_alive
from tools import linux_vm
from tests.process_guard_common import run_guarded_test_process
from tools.molt_dev_common import _run_fast_captured_command

ROOT = Path(__file__).resolve().parents[2]
VZ_BINARY = (
    "/System/Library/Frameworks/Virtualization.framework/Versions/A/XPCServices/"
    "com.apple.Virtualization.VirtualMachine.xpc/Contents/MacOS/"
    "com.apple.Virtualization.VirtualMachine"
)
DEAD_PID = 2**31  # beyond every POSIX pid, so never alive
# Measured on macOS 26.4 (Darwin 25.4.0), Lima 2.2.1, guests on Linux 6.8 and
# 7.0: the server closed forgotten descriptors at every 64th FUSE_FORGET.
SERVER_FORGET_BATCH = 64


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
    # Host files whose inode the guest caches and no guest process uses: a
    # drop makes the guest forget them.
    cached: list[HostFile] = field(default_factory=list)
    # Host files a guest process holds open: a drop keeps them.
    held_open: list[HostFile] = field(default_factory=list)
    # Forgotten files whose descriptors the server has not closed yet.
    forgotten: list[HostFile] = field(default_factory=list)
    guest_uid: int = 501
    guest_user: str = "molt"
    subuid: str = "molt:524288:65536"
    # (pid, ppid, uid, comm, cgroup, executable or "-")
    processes: list[tuple[int, int, int, str, str, str]] = field(default_factory=list)
    probe_shell: int = 4000
    probe_rc: int = 0
    next_fd: int = 100

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
        return files + self.cached + self.held_open + self.forgotten

    def forget(self, files: Sequence[HostFile]) -> None:
        for item in files:
            self.forgotten.append(item)
            if len(self.forgotten) == SERVER_FORGET_BATCH:
                self.forgotten = []

    def look_up(self, guest_glob: str) -> bool:
        """`stat -- <dir>/*` in the guest: one host descriptor per file."""
        assert guest_glob.endswith("/*"), guest_glob
        guest_dir = Path(guest_glob[:-2])
        for raw in self.config.get("mounts") or ():
            point, location = Path(raw["mountPoint"]), Path(raw["location"])
            if guest_dir.is_relative_to(point):
                host_dir = location / guest_dir.relative_to(point)
                break
        else:
            return False
        if not host_dir.is_dir():
            return False
        for path in sorted(host_dir.iterdir()):
            self.next_fd += 1
            self.cached.append(
                HostFile(str(path), path.stat().st_size, 1, str(self.next_fd))
            )
        return True

    def run_guest(self, script: str) -> bool:
        """The guest commands a release runs, step by step."""
        for step in script.split(" && "):
            if step == "echo 2 > /proc/sys/vm/drop_caches":
                self.forget(self.cached)
                self.cached = []
            elif step.startswith("stat -- ") and step.endswith(" >/dev/null"):
                words = shlex.split(step[len("stat -- ") : -len(" >/dev/null")])
                if not all(self.look_up(word) for word in words):
                    return False
            else:
                raise AssertionError(f"unexpected guest step {step!r}")
        return True


def probe_text(
    uid: int,
    user: str,
    shell: int,
    subuid: Sequence[str],
    processes: Sequence[tuple[int, int, int, str, str, str]],
) -> str:
    """What `IDLE_PROBE` prints in a guest with these processes."""
    lines = [str(uid), user, str(shell), "--- subuid", *subuid, "--- exe"]
    lines += [f"{pid:>7} {exe}" for pid, _pp, _u, _c, _g, exe in processes]
    lines.append("--- ps")
    lines += [
        f"{pid:>7} {ppid:>7} {uid:>5} {comm:<15} {cgroup}"
        for pid, ppid, uid, comm, cgroup, _exe in processes
    ]
    return "\n".join(lines) + "\n"


def probe_output(vm: FakeVm) -> str:
    return probe_text(
        vm.guest_uid, vm.guest_user, vm.probe_shell, [vm.subuid], vm.processes
    )


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
        # Commands run through `start_service`, in order (also in `calls`).
        self.services: list[tuple[str, ...]] = []
        self.env_overrides: list[dict[str, str]] = []
        self.validated: list[str] = []
        # Lima reads an instance's config from its lima.yaml on every list.
        self.configs_by_text: dict[str, dict[str, object]] = {}
        # Each `limactl start`: its exit status and the status Lima then
        # lists. A hostagent that dies after the command returns leaves
        # (0, "Stopped"), as on 2026-10-22 (HF-471).
        self.start_outcomes: list[tuple[int, str]] = []
        self.started_with: list[str] = []
        self.commits: dict[str, str] = {}
        self.checkout = linux_vm.Completed(0, "", "")

    def _listed(self, vm: FakeVm) -> str:
        text = (vm.directory / "lima.yaml").read_text(encoding="utf-8")
        if text in self.configs_by_text:
            vm.config = self.configs_by_text[text]
        return vm.json()

    def which(self, name: str) -> str | None:
        if name == "limactl" and not self.lima:
            return None
        return f"/fake/bin/{name}"

    def start_service(
        self, argv: Sequence[str], *, timeout: float
    ) -> linux_vm.Completed:
        argv = tuple(argv)
        self.calls.append(argv)
        self.services.append(argv)
        assert timeout > 0
        match argv:
            case ("limactl", "start", "--tty=false", name):
                vm = self.vms[name]
                self.started_with.append(
                    (vm.directory / "lima.yaml").read_text(encoding="utf-8")
                )
                rc, status = (
                    self.start_outcomes.pop(0)
                    if self.start_outcomes
                    else (0, "Running")
                )
                vm.status = status
                if status == "Running":
                    (vm.directory / "ha.pid").write_text("1\n", encoding="utf-8")
                    later = time.time() + 5
                    os.utime(vm.directory / "ha.pid", (later, later))
                    if self.on_start is not None:
                        self.on_start(vm)
                stderr = "" if rc == 0 else "INFO starting\nvz: boot failed\n"
                return linux_vm.Completed(rc, "", stderr)
        raise AssertionError(f"unexpected service {argv}")

    def run(
        self,
        argv: Sequence[str],
        *,
        timeout: float,
        env_overrides: dict[str, str] | None = None,
    ) -> linux_vm.Completed:
        argv = tuple(argv)
        self.calls.append(argv)
        self.env_overrides.append(dict(env_overrides or {}))
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
                vm = self.vms[name]
                assert vm.status == "Running"
                if self.drop_rc:
                    return linux_vm.Completed(
                        self.drop_rc, "", "sudo: a password is required"
                    )
                ok = vm.run_guest(script)
                return linux_vm.Completed(0 if ok else 1, "", "" if ok else "stat")
            case ("limactl", "shell", "--workdir", "/", name, "sh", "-c", script):
                vm = self.vms[name]
                if script.startswith("set -u\n"):
                    return self.checkout
                assert script == linux_vm.IDLE_PROBE, script
                if vm.probe_rc:
                    return linux_vm.Completed(
                        vm.probe_rc, "", "sudo: a password is required"
                    )
                return linux_vm.Completed(0, probe_output(vm), "")
            case ("limactl", "stop", name):
                self.vms[name].status = "Stopped"
                return linux_vm.Completed(0, "", "")
            case ("git", "-C", _repo, "rev-parse", "--verify", "--quiet", rev):
                commit = self.commits.get(rev.removesuffix("^{commit}"), "")
                return linux_vm.Completed(0 if commit else 1, commit + "\n", "")
            case ("git", "-C", _repo, "push", *_rest):
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

    Written from `limactl validate --fill` output (Lima 2.2.1): an empty
    `mounts` list disappears, Lima drops `ignore: false`, and every other
    rendered key survives as given.
    """
    config = json.loads(json.dumps(rendered))
    if config.get("mounts") == []:
        del config["mounts"]
    for rule in config.get("portForwards", []):
        if rule.get("ignore") is False:
            del rule["ignore"]
    config["containerd"]["archives"] = [
        {"location": "https://example.invalid/nerdctl.tgz"}
    ]
    config["ssh"] = {"localPort": 0}
    return config


def mounted(location: Path, point: Path | None = None) -> dict[str, object]:
    """A virtiofs mount as `limactl list --json` shows it."""
    return {
        "location": str(location),
        "mountPoint": str(point or location),
        "writable": False,
        "sshfs": {"cache": True, "followSymlinks": False, "sftpDriver": ""},
        "9p": {"securityModel": "none", "cache": "fscache"},
    }


@pytest.fixture
def definitions() -> linux_vm.Definitions:
    return linux_vm.load_definitions()


def make_instance_dir(
    base: Path, name: str, *, started_after_config: bool = True, text: str = ""
) -> Path:
    directory = base / name
    directory.mkdir(parents=True)
    (directory / "lima.yaml").write_text(
        text or '# hand-made\nmounts:\n- location: "~"\n', encoding="utf-8"
    )
    (directory / "ha.pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
    (directory / "ssh.config").write_text(f"Host lima-{name}\n", encoding="utf-8")
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
    **overrides: object,
) -> FakeVm:
    vm = definitions.vms[name]
    rendered = linux_vm.lima_config(
        vm, minimum_lima_version=definitions.minimum_lima_version
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


def deleted_names(host: FakeHost, vm: FakeVm) -> list[str]:
    """The deleted regular files the VM process holds, by name, from lsof."""
    completed = host.run(
        ["lsof", "-n", "-P", "-a", "+L1", "-p", str(vm.vz_pid), "-F", "ftn"],
        timeout=10.0,
    )
    names, kind = [], ""
    for line in completed.stdout.splitlines():
        if line.startswith("t"):
            kind = line[1:]
        elif line.startswith("n") and kind == "REG":
            names.append(line[1:])
    return sorted(set(names) - {"/Library/Preferences/Logging/.plist-cache.X"})


# --------------------------------------------------------------------------
# Definitions
# --------------------------------------------------------------------------


def test_checked_in_vms_mount_nothing_forward_nothing_and_name_a_clone(
    definitions: linux_vm.Definitions,
) -> None:
    assert set(definitions.vms) == {"molt-linux", "molt-x86"}
    for vm in definitions.vms.values():
        assert linux_vm.lima_config(vm, minimum_lima_version="2.0.0")["mounts"] == []
        assert vm.port_forwards == ()
        assert linux_vm.port_rules(vm)[-1] == linux_vm.IGNORE_EVERY_GUEST_PORT
        assert vm.clone == "molt-main"
    assert definitions.vms["molt-x86"].rosetta
    assert not definitions.vms["molt-linux"].rosetta
    # Idle by executable path; a shim runs only while a container runs: it
    # is work, not idleness.
    assert all(path.startswith("/") for path in definitions.idle_guest_executables)
    assert not any("shim" in path for path in definitions.idle_guest_executables)
    text = linux_vm.DEFINITIONS_PATH.read_text(encoding="utf-8")
    assert "/Users/" not in text and "/home/" not in text, "no personal host path"


_VALID = """\
schema = 3
minimum_lima_version = "2.0.0"
idle_guest_executables = ["/usr/lib/systemd/systemd"]
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
params = {}
port_forwards = []
clone = "molt-main"

[vm.a.image]
location = "https://example.invalid/a.img"
digest = "sha256:%s"
""" % ("0" * 64)


@pytest.mark.parametrize(
    ("old", "new", "message"),
    [
        ("cpus = 2", "cpus = 2\nextra = 1", "unknown ['extra']"),
        ('memory = "4GiB"', 'memory = "4GB"', "must look like 12GiB"),
        # The schema no longer declares mounts: a definition cannot add one.
        (
            "port_forwards = []",
            'port_forwards = []\nmounts = [{ source = "custody-root" }]',
            "unknown ['mounts']",
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
        ('clone = "molt-main"', 'clone = "../escape"', "without '..'"),
        ('clone = "molt-main"', 'clone = "/abs/path"', "without '..'"),
        ('digest = "sha256:', 'digest = "md5:', "sha256:<64 hex>"),
        ("schema = 3", "schema = 2", "schema must be 3"),
        # A process name proves nothing: idleness is an executable path.
        ('["/usr/lib/systemd/systemd"]', '["systemd"]', "not an absolute path"),
        ('["/usr/lib/systemd/systemd"]', '["/usr/../tmp/x"]', "contains '..'"),
        (
            '["/usr/lib/systemd/systemd"]',
            '["/usr/lib/systemd/systemd", "/usr/lib/systemd/systemd"]',
            "lists a path twice",
        ),
        (
            "idle_guest_executables",
            "idle_guest_processes",
            "unknown ['idle_guest_processes']",
        ),
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
    config = linux_vm.lima_config(vm, minimum_lima_version="2.0.0")
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
    text = linux_vm.render(vm, minimum_lima_version="2.0.0")
    assert "\nmounts: []\n" in text
    assert '  - guestIP: "127.0.0.1"\n' in text
    assert "    ignore: true\n" in text
    assert text.index("18080") < text.index("ignore: true")


@pytest.mark.skipif(shutil.which("limactl") is None, reason="Lima is not installed")
@pytest.mark.parametrize("name", ["molt-linux", "molt-x86"])
def test_lima_parses_the_rendered_config_as_the_definition_says(
    definitions: linux_vm.Definitions, tmp_path: Path, name: str
) -> None:
    """Lima's own parser (`limactl validate --fill`) reads the rendered YAML.

    Lima adds no mount of its own (its template's home and /tmp/lima mounts
    apply only when a config omits `mounts`).
    """
    vm = definitions.vms[name]
    path = tmp_path / f"{name}.yaml"
    path.write_text(
        linux_vm.render(vm, minimum_lima_version=definitions.minimum_lima_version),
        encoding="utf-8",
    )
    filled = linux_vm.LocalHost({**os.environ, "LIMA_HOME": str(tmp_path)}).run(
        ["limactl", "validate", "--fill", str(path)], timeout=60.0
    )
    assert filled.returncode == 0, filled.stderr
    text = filled.stdout
    assert f"cpus: {vm.cpus}\n" in text
    assert f"memory: {vm.memory.text}\n" in text
    assert "\nmounts:" not in text and "mountPoint" not in text
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
        ("molt-linux", _set("mounts", [mounted(Path("/Users/x"))]), {"mounts"}),
        ("molt-x86", _set("mounts", [mounted(Path("/Users/x/Molt"))]), {"mounts"}),
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
    fake = matching_vm(definitions, name, tmp_path / "lima")
    if edit is not None:
        edit(fake.config)
    drift = linux_vm.compare(definitions.vms[name], instance_of(fake))
    assert {str(item.field) for item in drift} == fields


def test_compare_reads_an_extra_forward_rule_ahead_of_the_ignore_rule_as_drift(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    fake = matching_vm(definitions, "molt-linux", tmp_path / "lima")
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
    drift = linux_vm.compare(definitions.vms["molt-linux"], instance_of(fake))
    assert [item.field for item in drift] == [linux_vm.DriftField.PORT_FORWARDS]
    assert "4226" in drift[0].effective


def test_compare_reads_resources_from_the_instance(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    fake = matching_vm(
        definitions,
        "molt-x86",
        tmp_path / "lima",
        cpus=2,
        memory=7 << 30,
        disk=100 << 30,
    )
    drift = linux_vm.compare(definitions.vms["molt-x86"], instance_of(fake))
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
    fake = matching_vm(definitions, "molt-linux", tmp_path / "lima", status=status)
    shutil.rmtree(fake.directory)
    fake.directory = make_instance_dir(
        tmp_path / "lima", "molt-linux", started_after_config=started_after_config
    )
    drift = linux_vm.compare(definitions.vms["molt-linux"], instance_of(fake))
    assert [item.field for item in drift] == (
        [linux_vm.DriftField.RESTART_PENDING] if pending else []
    )


# --------------------------------------------------------------------------
# Held files
# --------------------------------------------------------------------------


def two_vms(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> tuple[FakeVm, FakeVm, Path]:
    """Two running VMs that still mount host paths, as before `apply`."""
    custody = tmp_path / "custody"
    custody.mkdir()
    linux = matching_vm(definitions, "molt-linux", tmp_path / "lima", vz_pid=9200)
    x86 = matching_vm(definitions, "molt-x86", tmp_path / "lima", vz_pid=9100)
    # molt-x86 mounts the custody root; molt-linux mounts its parent.
    x86.config["mounts"] = [mounted(custody)]
    linux.config["mounts"] = [mounted(tmp_path)]
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
    linux, x86, custody = two_vms(definitions, tmp_path)
    x86.cached.append(HostFile(str(custody / linux_vm.FORGET_SET / "00"), 0, 1, "30"))
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
        # The forget set's live files are the release's own.
        "molt-x86": linux_vm.Held(
            pid=9100, deleted_files=0, deleted_bytes=0, open_under_mounts=1
        ),
    }
    assert [call[0] for call in host.calls] == ["limactl", "lsof"]


# --------------------------------------------------------------------------
# Release: the mechanism
# --------------------------------------------------------------------------


def test_a_drop_alone_frees_nothing_and_the_release_frees_every_cached_pin(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    """The server closes forgotten descriptors only at its 64th forget.

    A bare `drop_caches` makes the guest forget three deleted files, and the
    server keeps all three; HF-463's first release stopped there. The release
    adds 64 forgets of its own, so every deleted file the guest does not hold
    open is freed; the one a guest process holds stays.
    """
    linux, x86, custody = two_vms(definitions, tmp_path)
    host = FakeHost([linux, x86])
    pinned = [
        str(custody / "worktrees/old/a.py"),
        "/3f1c0ffee",
        str(custody / "tmp/log"),
    ]
    assert deleted_names(host, linux) == sorted(pinned)
    bare = host.run(
        [
            "limactl",
            "shell",
            "--workdir",
            "/",
            "molt-linux",
            "sudo",
            "-n",
            "sh",
            "-c",
            linux_vm.DROP_GUEST_CACHES,
        ],
        timeout=10.0,
    )
    assert bare.returncode == 0
    assert deleted_names(host, linux) == sorted(pinned), "a drop alone frees nothing"
    results = linux_vm.release(host, linux_vm.list_instances(host))
    assert deleted_names(host, linux) == [str(custody / "tmp/log")]
    by_name = {result.name: result for result in results}
    assert by_name["molt-linux"].before.deleted_files == 3
    assert by_name["molt-linux"].after == linux_vm.Held(
        pid=9200, deleted_files=1, deleted_bytes=7, open_under_mounts=0
    )
    assert by_name["molt-x86"].after.open_under_mounts == 0
    assert all(result.error is None for result in results)
    # Each mount got its forget set on the host.
    for location in (tmp_path, custody):
        assert sorted(os.listdir(location / linux_vm.FORGET_SET)) == [
            f"{index:02d}" for index in range(linux_vm.FORGET_BATCH)
        ]


@pytest.mark.parametrize("pending", [0, 1, 40, 63])
def test_the_release_frees_pins_whatever_the_server_already_has_pending(
    definitions: linux_vm.Definitions, tmp_path: Path, pending: int
) -> None:
    """Earlier forgets leave 0 to 63 pending; the release must not depend on it."""
    linux, x86, custody = two_vms(definitions, tmp_path)
    linux.forgotten = [
        HostFile(str(custody / f"old/{index}"), 1, 1, str(500 + index))
        for index in range(pending)
    ]
    host = FakeHost([linux, x86])
    linux_vm.release(host, linux_vm.list_instances(host, ["molt-linux"]))
    assert deleted_names(host, linux) == [str(custody / "tmp/log")]
    assert not any(
        item.name.startswith(str(custody / "old")) for item in linux.forgotten
    )


def test_release_skips_vms_that_mount_nothing_and_never_stops_a_vm(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    linux, x86, _custody = two_vms(definitions, tmp_path)
    applied = matching_vm(definitions, "molt-x86", tmp_path / "other")
    applied.name = "applied-vm"
    stopped = matching_vm(
        definitions, "molt-x86", tmp_path / "stopped", status="Stopped"
    )
    stopped.name = "stopped-vm"
    stopped.config["mounts"] = [mounted(tmp_path)]
    host = FakeHost([linux, x86, applied, stopped])
    results = linux_vm.release(host, linux_vm.list_instances(host))
    assert {result.name for result in results} == {"molt-linux", "molt-x86"}
    guest = [call for call in host.calls if call[:2] == ("limactl", "shell")]
    assert sorted(call[4] for call in guest) == ["molt-linux", "molt-x86"]
    assert not [call for call in host.calls if call[1:2] in (("stop",), ("start",))]


def test_release_command_reports_a_failed_guest_command(
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


def test_an_unwritable_mount_fails_alone_and_the_others_release(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """A-5: one mount without a forget set does not stop the VM's release.

    The server counts forgets per VM process, so the other mount's 64
    forgets free every pin; the failed mount is still reported, and the
    command exits 1.
    """
    linux, x86, custody = two_vms(definitions, tmp_path)
    missing = tmp_path / "missing"
    linux.config["mounts"] = [mounted(missing), mounted(tmp_path)]
    host = FakeHost([linux, x86])
    [result] = linux_vm.release(host, linux_vm.list_instances(host, ["molt-linux"]))
    assert result.error is None
    assert len(result.mount_errors) == 1 and str(missing) in result.mount_errors[0]
    [guest] = [call for call in host.calls if call[1:2] == ("shell",)]
    assert f"{tmp_path}/{linux_vm.FORGET_SET}/*" in guest[-1]
    assert str(missing) not in guest[-1]
    assert deleted_names(host, linux) == [str(custody / "tmp/log")]
    assert linux_vm.release_command(host=host, names=["molt-linux"], as_json=True) == 1
    [row] = json.loads(capsys.readouterr().out)["vms"]
    assert row["error"] is None and str(missing) in row["mount_errors"][0]


def test_a_vm_with_no_writable_mount_runs_nothing_in_the_guest(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    linux, x86, _custody = two_vms(definitions, tmp_path)
    linux.config["mounts"] = [mounted(tmp_path / "missing")]
    host = FakeHost([linux, x86])
    [result] = linux_vm.release(host, linux_vm.list_instances(host, ["molt-linux"]))
    assert result.error == "no mount has a forget set"
    assert len(result.mount_errors) == 1
    assert not [call for call in host.calls if call[1:2] == ("shell",)]
    assert not (linux.directory / linux_vm.RELEASE_RECORD).exists()


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


def _applied(definitions: linux_vm.Definitions, home: Path, name: str) -> FakeVm:
    text = linux_vm.render(
        definitions.vms[name], minimum_lima_version=definitions.minimum_lima_version
    )
    fake = matching_vm(definitions, name, home)
    shutil.rmtree(fake.directory)
    fake.directory = make_instance_dir(home, name, text=text)
    return fake


def test_the_hook_spawns_nothing_when_every_live_vm_runs_its_definition(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    """After `apply` the VMs mount nothing, so retirement costs file reads."""
    home = tmp_path / "lima"
    linux = _applied(definitions, home, "molt-linux")
    x86 = _applied(definitions, home, "molt-x86")
    host = FakeHost([linux, x86])
    environment = {"LIMA_HOME": str(home)}
    assert (
        linux_vm.release_for_paths([tmp_path], environment=environment, host=host) == ()
    )
    assert host.calls == []
    # Any doubt asks Lima: a config edited after start, a shared Lima config,
    # or an instance no definition names.
    os.utime(linux.directory / "lima.yaml", (time.time() + 60, time.time() + 60))
    assert (
        linux_vm.release_for_paths([tmp_path], environment=environment, host=host) == ()
    )
    assert host.calls == [("limactl", "list", "--json")]
    os.utime(linux.directory / "lima.yaml", (time.time() - 600, time.time() - 600))
    (home / "_config").mkdir()
    (home / "_config" / "override.yaml").write_text("mounts: []\n", encoding="utf-8")
    host.calls.clear()
    linux_vm.release_for_paths([tmp_path], environment=environment, host=host)
    assert host.calls == [("limactl", "list", "--json")]


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


def test_the_hook_skips_the_release_when_no_covering_vm_holds_files_there(
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
    # Only molt-linux's mount (the custody root's parent) covers it.
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


def test_a_retirement_after_a_release_runs_nothing_in_a_guest(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    """B2: a release that frees its pins leaves later retirements nothing to do.

    The first release frees every deleted file but the one a guest process
    holds; with that process gone, a retirement of a path the guest never
    read spawns no guest command.
    """
    linux, x86, custody = two_vms(definitions, tmp_path)
    linux.held_open = []
    host = FakeHost([linux, x86])
    environment = {"LIMA_HOME": str(tmp_path / "lima")}
    first = linux_vm.release_for_paths(
        [custody / "worktrees" / "old"], environment=environment, host=host
    )
    assert [(r.name, r.after.deleted_files) for r in first] == [("molt-linux", 0)]
    host.calls.clear()
    assert (
        linux_vm.release_for_paths(
            [custody / "worktrees" / "fresh"], environment=environment, host=host
        )
        == ()
    )
    assert [call[0] for call in host.calls] == ["limactl", "lsof"]


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


def test_the_hook_skips_a_release_that_cannot_free_what_a_guest_holds_open(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    """A-1: a deleted file a guest process holds open survives every release.

    The first release records what it left. A later retirement whose VM
    holds exactly that skips the release; a new pin, a closed file or a new
    VM process each make the release worth running again.
    """
    linux, x86, custody = two_vms(definitions, tmp_path)
    x86.vz_pid = None
    log = HostFile(str(custody / "tmp/log"), 7, 0, "15")
    log2 = HostFile(str(custody / "tmp/log2"), 9, 0, "17")
    linux.held_open = [log, log2]
    host = FakeHost([linux, x86])
    environment = {"LIMA_HOME": str(tmp_path / "lima")}

    def retire(path: str) -> list[str]:
        host.calls.clear()
        linux_vm.release_for_paths(
            [custody / "worktrees" / path], environment=environment, host=host
        )
        return [call[1] if call[0] == "limactl" else call[0] for call in host.calls]

    assert retire("one") == ["list", "lsof", "shell", "lsof"]
    assert deleted_names(host, linux) == sorted([log.name, log2.name])
    record = json.loads(
        (linux.directory / linux_vm.RELEASE_RECORD).read_text(encoding="utf-8")
    )
    assert record["pid"] == 9200 and len(record["deleted"]) == 2
    # Nothing changed: no guest command.
    assert retire("two") == ["list", "lsof"]
    # A new pin: release, and the new record holds the open files again.
    linux.cached.append(HostFile(str(custody / "worktrees/two/x"), 3, 0, "40"))
    assert retire("three") == ["list", "lsof", "shell", "lsof"]
    assert retire("four") == ["list", "lsof"]
    # The guest closed one file: fewer descriptors, so its lookup can go.
    linux.held_open = [log]
    assert retire("five") == ["list", "lsof", "shell", "lsof"]
    assert retire("six") == ["list", "lsof"]
    # A restarted VM process holds new descriptors: the record is stale.
    linux.vz_pid = 9300
    assert retire("seven") == ["list", "lsof", "shell", "lsof"]
    # An unreadable record never skips a release.
    (linux.directory / linux_vm.RELEASE_RECORD).write_text("{", encoding="utf-8")
    assert retire("eight") == ["list", "lsof", "shell", "lsof"]


# --------------------------------------------------------------------------
# Push
# --------------------------------------------------------------------------


def test_push_sends_the_resolved_commit_over_limas_ssh_and_checks_it_out(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    fake = matching_vm(definitions, "molt-linux", tmp_path / "lima")
    host = FakeHost([fake])
    host.commits["HEAD"] = "c" * 40
    rc = linux_vm.push(
        "molt-linux",
        rev="HEAD",
        ref=linux_vm.DEFAULT_PUSH_REF,
        checkout=True,
        definitions=definitions,
        host=host,
        repo=tmp_path / "repo",
    )
    assert rc == 0, capsys.readouterr().out
    [push_index] = [i for i, call in enumerate(host.calls) if "push" in call]
    config = shlex.quote(str(fake.directory / "ssh.config"))
    assert host.calls[push_index] == (
        "git",
        "-C",
        str(tmp_path / "repo"),
        "push",
        "--force",
        "--no-verify",
        "--quiet",
        "lima-molt-linux:molt-main",
        f"{'c' * 40}:refs/heads/host/head",
    )
    # GIT_SSH_COMMAND outranks every other ssh setting Git reads.
    assert host.env_overrides[push_index] == {
        "GIT_SSH_COMMAND": f"ssh -F {config}",
        "GIT_SSH_VARIANT": "ssh",
    }
    checkout = host.calls[-1]
    assert checkout[:5] == ("limactl", "shell", "--workdir", "/", "molt-linux")
    assert checkout[-1] == linux_vm.checkout_script("molt-main", "c" * 40)


def test_push_reports_a_refused_checkout_with_the_exact_commit(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    fake = matching_vm(definitions, "molt-linux", tmp_path / "lima")
    host = FakeHost([fake])
    host.commits["HEAD"] = "c" * 40
    host.checkout = linux_vm.Completed(
        linux_vm.CHECKOUT_REFUSED, "processes work in /home/m/molt-main (pid 7)\n", ""
    )
    common = dict(rev="HEAD", ref="lane/x", definitions=definitions, host=host)
    assert linux_vm.push("molt-linux", checkout=True, **common) == 1
    out = capsys.readouterr().out
    assert "REFUSED: checkout in molt-linux: processes work" in out
    assert "waits as refs/heads/host/lane/x" in out
    assert linux_vm.push("molt-linux", checkout=False, **common) == 0
    assert f"git -C ~/molt-main checkout --detach {'c' * 40}" in capsys.readouterr().out


@pytest.mark.parametrize(
    ("ref", "full"),
    [
        ("head", "refs/heads/host/head"),
        ("lane/vm-followups", "refs/heads/host/lane/vm-followups"),
        ("v1.0_rc-2", "refs/heads/host/v1.0_rc-2"),
        ("../x", None),
        ("a..b", None),
        (".hidden", None),
        ("x.lock", None),
        ("x.", None),
        ("x/", None),
        ("/x", None),
        ("a//b", None),
        ("-x", None),
        ("", None),
        ("x y", None),
        ("x@{1}", None),
    ],
)
def test_push_confines_every_ref_to_the_host_namespace(
    ref: str, full: str | None
) -> None:
    """A-3: a forced push can move only a branch the host owns."""
    assert linux_vm.push_ref(ref) == full


@pytest.mark.parametrize(
    ("change", "rc"),
    [("stopped", 1), ("no-commit", 2), ("bad-ref", 2), ("undefined", 2)],
)
def test_push_refuses_before_it_sends_anything(
    definitions: linux_vm.Definitions, tmp_path: Path, change: str, rc: int
) -> None:
    fake = matching_vm(definitions, "molt-linux", tmp_path / "lima")
    host = FakeHost([fake])
    host.commits["HEAD"] = "c" * 40
    name, ref, rev = "molt-linux", linux_vm.DEFAULT_PUSH_REF, "HEAD"
    if change == "stopped":
        fake.status = "Stopped"
    elif change == "no-commit":
        rev = "nope"
    elif change == "bad-ref":
        ref = "../x"
    else:
        name = "molt-other"
    assert (
        linux_vm.push(
            name,
            rev=rev,
            ref=ref,
            checkout=False,
            definitions=definitions,
            host=host,
            repo=tmp_path,
        )
        == rc
    )
    assert not [call for call in host.calls if "push" in call]


class GitHost(FakeHost):
    """Runs `git` and the guest's checkout command for real.

    `git` runs with a fake `ssh` first on PATH; the guest command runs in a
    shell whose HOME is the guest's home, with a fake `sudo` whose `find`
    prints the `/proc/<pid>/cwd` listing in ``proc`` (macOS has no /proc).
    ``environment`` is the host environment `run` overrides from.
    """

    def __init__(
        self, vms: Sequence[FakeVm], environment: dict[str, str], home: Path
    ) -> None:
        super().__init__(vms)
        self.environment = environment
        self.home = home
        self.proc = home.parent / "proc-cwd.txt"
        self.proc.write_text("", encoding="utf-8")

    def _real(
        self, argv: list[str], env: dict[str, str], timeout: float
    ) -> linux_vm.Completed:
        # The guarded test runner, not the plumbing helper, whose transfer
        # bound waits for progress.
        completed = run_guarded_test_process(
            argv, env=ProcessEnvironment(env), timeout=timeout
        )
        return linux_vm.Completed(
            completed.returncode, completed.stdout, completed.stderr
        )

    def run(
        self,
        argv: Sequence[str],
        *,
        timeout: float,
        env_overrides: dict[str, str] | None = None,
    ) -> linux_vm.Completed:
        argv = list(argv)
        if argv[0] == "git":
            self.calls.append(tuple(argv))
            self.env_overrides.append(dict(env_overrides or {}))
            return self._real(
                argv, {**self.environment, **(env_overrides or {})}, timeout
            )
        if argv[:5] == ["limactl", "shell", "--workdir", "/", "molt-linux"] and (
            argv[-1].startswith("set -u\n")
        ):
            self.calls.append(tuple(argv))
            guest = {
                **self.environment,
                "HOME": str(self.home),
                "FAKE_PROC_CWD": str(self.proc),
            }
            # The guests' /bin/sh is dash; macOS ships one too.
            shell = shutil.which("dash") or "sh"
            return self._real([shell, "-c", argv[-1]], guest, timeout)
        return super().run(argv, timeout=timeout, env_overrides=env_overrides)


_GIT = [
    "git",
    "-c",
    "user.name=t",
    "-c",
    "user.email=t@t",
    "-c",
    "init.defaultBranch=main",
    "-c",
    "core.hooksPath=/dev/null",
]


def run_git(*args: str) -> str:
    completed = _run_fast_captured_command([*_GIT, *args])
    assert completed.returncode == 0, completed.stderr
    return completed.stdout.strip()


def git_guest(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> tuple[GitHost, Path, Path, str]:
    """A host repository with one commit and a guest home with an empty clone.

    The fake `ssh` takes OpenSSH's form (options, the host, the remote
    command) and requires the instance's ssh.config; the fake `sudo` serves
    `sudo -n true` and the checkout's `sudo -n find /proc ...`. The push runs
    under the guard, whose Git transfer bound wraps `GIT_SSH_COMMAND`
    (tools/git_transfer.py) and keeps the command it wraps.
    """
    repo, home = tmp_path / "repo", tmp_path / "guest-home"
    for directory in (repo, home / "molt-main"):
        run_git("init", "-q", str(directory))
    (repo / "a.txt").write_text("a\n", encoding="utf-8")
    run_git("-C", str(repo), "add", "a.txt")
    run_git("-C", str(repo), "commit", "-qm", "a")
    head = run_git("-C", str(repo), "rev-parse", "HEAD")
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    ssh_config = tmp_path / "lima" / "molt-linux" / "ssh.config"
    (bin_dir / "ssh").write_text(
        f"""#!{sys.executable}
import subprocess, sys
args = sys.argv[1:]
if "-G" in args:
    # Git probes the ssh variant with `ssh -G <host>`; OpenSSH prints the
    # resolved configuration and exits 0.
    sys.exit(0)
# The guard's transfer bound may put its own -o options first.
assert args[args.index("-F") + 1] == {str(ssh_config)!r}, args
# TortoisePlink's flag; OpenSSH would read it as `-b atch`.
assert "-batch" not in args, args
while args[0].startswith("-"):
    args = args[2:] if args[0] in ("-F", "-o", "-p") else args[1:]
host, command = args[0], " ".join(args[1:])
assert host == "lima-molt-linux", host
sys.exit(subprocess.run(["sh", "-c", command], cwd={str(home)!r}).returncode)
""",
        encoding="utf-8",
    )
    (bin_dir / "sudo").write_text(
        """#!/bin/sh
[ "$1" = -n ] || exit 1
shift
case "$1" in
  true) exit 0 ;;
  find) [ "$2" = /proc ] || exit 1; cat "$FAKE_PROC_CWD" ;;
  *) exit 1 ;;
esac
""",
        encoding="utf-8",
    )
    for tool in ("ssh", "sudo"):
        (bin_dir / tool).chmod(0o755)
    fake = matching_vm(definitions, "molt-linux", tmp_path / "lima")
    environment = {
        **os.environ,
        "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
    }
    return GitHost([fake], environment, home), repo, home / "molt-main", head


def test_push_moves_a_commit_into_the_guest_clone_through_real_git(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    """Real Git runs the push; a fake `ssh` runs the guest side in a directory.

    The oracle is the guest clone's own refs, read by Git after the push. A
    `GIT_SSH_COMMAND` or `GIT_SSH_VARIANT` in the caller's environment
    outranks `core.sshCommand` (A-2); the push sets its own.
    """
    host, repo, clone, head = git_guest(definitions, tmp_path)
    host.environment.update(
        {"GIT_SSH_COMMAND": "false", "GIT_SSH_VARIANT": "tortoiseplink"}
    )
    # A guest branch of the same name is not the host's to move (A-3).
    (clone / "g.txt").write_text("g\n", encoding="utf-8")
    run_git("-C", str(clone), "add", "g.txt")
    run_git("-C", str(clone), "commit", "-qm", "guest work")
    guest_main = run_git("-C", str(clone), "rev-parse", "refs/heads/main")
    run_git("-C", str(clone), "checkout", "-q", "--detach")
    assert (
        linux_vm.push(
            "molt-linux",
            rev="HEAD",
            ref="main",
            checkout=False,
            definitions=definitions,
            host=host,
            repo=repo,
        )
        == 0
    )
    assert run_git("-C", str(clone), "rev-parse", "refs/heads/host/main") == head
    assert run_git("-C", str(clone), "rev-parse", "refs/heads/main") == guest_main


@pytest.mark.parametrize(
    "state", ["clean", "untracked", "modified", "index-lock", "cwd", "no-sudo"]
)
def test_push_checks_out_the_exact_commit_only_into_a_clean_idle_clone(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    state: str,
) -> None:
    """A-4: the guest's own checkout command, run for real against a clone.

    Untracked or modified files, a held index lock, or a process whose
    working directory lies in the clone each refuse the checkout and leave
    the clone where it was; a guest without passwordless sudo fails it. Only
    a clean, idle clone moves, detached, to the pushed commit.
    """
    host, repo, clone, head = git_guest(definitions, tmp_path)
    (clone / "g.txt").write_text("g\n", encoding="utf-8")
    run_git("-C", str(clone), "add", "g.txt")
    run_git("-C", str(clone), "commit", "-qm", "guest base")
    before = run_git("-C", str(clone), "rev-parse", "HEAD")
    resolved = clone.resolve()
    # Processes elsewhere, one in a sibling whose path shares the prefix.
    lines = ["/proc/1 /", f"/proc/2 {resolved}-other", "/proc/3 /tmp"]
    if state == "untracked":
        (clone / "scratch.txt").write_text("x\n", encoding="utf-8")
    elif state == "modified":
        (clone / "g.txt").write_text("changed\n", encoding="utf-8")
    elif state == "index-lock":
        (clone / ".git" / "index.lock").write_text("", encoding="utf-8")
    elif state == "cwd":
        lines.append(f"/proc/4242 {resolved}/target/debug")
    elif state == "no-sudo":
        (tmp_path / "bin" / "sudo").write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
    host.proc.write_text("".join(line + "\n" for line in lines), encoding="utf-8")
    rc = linux_vm.push(
        "molt-linux",
        rev="HEAD",
        ref=linux_vm.DEFAULT_PUSH_REF,
        checkout=True,
        definitions=definitions,
        host=host,
        repo=repo,
    )
    out = capsys.readouterr().out
    assert run_git("-C", str(clone), "rev-parse", "refs/heads/host/head") == head
    after = run_git("-C", str(clone), "rev-parse", "HEAD")
    if state == "clean":
        assert rc == 0, out
        assert after == head
        detached = _run_fast_captured_command(
            ["git", "-C", str(clone), "symbolic-ref", "-q", "HEAD"]
        )
        assert detached.returncode == 1, "the checkout is detached"
        return
    assert rc == 1, out
    assert after == before
    expected = {
        "untracked": "scratch.txt",
        "modified": "g.txt",
        "index-lock": "index.lock exists",
        "cwd": "(pid 4242)",
        "no-sudo": "FAILED: checkout in molt-linux",
    }[state]
    assert expected in out, out
    if state != "no-sudo":
        assert "REFUSED: checkout in molt-linux" in out


# --------------------------------------------------------------------------
# Apply
# --------------------------------------------------------------------------

USER = "0::/user.slice/user-501.slice/user@501.service"
SESSION_CGROUP = "0::/user.slice/user-501.slice/session-4.scope"
SYSTEMD = "/usr/lib/systemd/systemd"
SSHD_SESSION = "/usr/lib/openssh/sshd-session"
# What both guests showed on 2026-10-22, as root sees it: Lima's SSH session,
# the user's systemd and its rootless containerd stack.
SESSION = [
    (1, 0, 0, "systemd", "0::/init.scope", SYSTEMD),
    (2, 0, 0, "kthreadd", "0::/", "-"),
    (
        300,
        1,
        0,
        "lima-guestagent",
        "0::/system.slice/lima-guestagent.service",
        "/usr/local/bin/lima-guestagent",
    ),
    # unattended-upgrade-shutdown, resident
    (
        310,
        1,
        0,
        "unattended-upgr",
        "0::/system.slice/unattended-upgrades.service",
        "/usr/bin/python3.12",
    ),
    (320, 1, 0, "cron", "0::/system.slice/cron.service", "/usr/sbin/cron"),
    (900, 1, 501, "systemd", f"{USER}/init.scope", SYSTEMD),
    (
        901,
        900,
        501,
        "(sd-pam)",
        f"{USER}/init.scope",
        "/usr/lib/systemd/systemd-executor",
    ),
    (
        940,
        900,
        501,
        "rootlesskit",
        f"{USER}/app.slice/containerd.service",
        "/usr/local/bin/rootlesskit",
    ),
    # rootlesskit's child re-executes /proc/self/exe: its name is `exe`.
    (
        945,
        940,
        501,
        "exe",
        f"{USER}/app.slice/containerd.service",
        "/usr/local/bin/rootlesskit",
    ),
    (
        950,
        945,
        501,
        "containerd",
        f"{USER}/app.slice/containerd.service",
        "/usr/local/bin/containerd",
    ),
    (
        960,
        900,
        501,
        "buildkitd",
        f"{USER}/app.slice/default-buildkit.service",
        "/usr/local/bin/buildkitd",
    ),
    (
        961,
        900,
        501,
        "containerd-star",
        f"{USER}/app.slice/stargz-snapshotter.service",
        "/usr/local/bin/containerd-stargz-grpc",
    ),
    (3900, 1, 0, "sshd-session", SESSION_CGROUP, SSHD_SESSION),
    (3901, 3900, 501, "sshd-session", SESSION_CGROUP, SSHD_SESSION),
    (3950, 3901, 501, "bash", SESSION_CGROUP, "/usr/bin/bash"),  # login shell
    (4000, 3950, 501, "sh", SESSION_CGROUP, "/usr/bin/dash"),  # the probe shell
    # The probe's own commands: `sudo ps` and its child, then `ps`.
    (4001, 4000, 0, "sudo", SESSION_CGROUP, "/usr/bin/sudo"),
    (4002, 4001, 0, "ps", SESSION_CGROUP, "/usr/bin/ps"),
    (4003, 4000, 501, "ps", SESSION_CGROUP, "/usr/bin/ps"),
]


def _probe(processes: Sequence[tuple[int, int, int, str, str, str]]) -> str:
    return probe_text(
        501, "molt", 4000, ["other:100000:65536", "molt:524288:65536"], processes
    )


def test_busy_guest_processes_exempts_this_session_and_guest_daemons(
    definitions: linux_vm.Definitions,
) -> None:
    assert linux_vm.busy_guest_processes(_probe(SESSION), definitions) == ()
    shell = "/usr/bin/bash"
    busy = SESSION + [
        (5000, 1, 501, "sleep", SESSION_CGROUP, "/usr/bin/sleep"),  # detached
        (5100, 1, 0, "dpkg", "0::/system.slice/apt.service", "/usr/bin/dpkg"),
        # Root work from a session.
        (5200, 3901, 0, "sudo", SESSION_CGROUP, "/usr/bin/sudo"),
        (5201, 5200, 0, "make", SESSION_CGROUP, "/usr/bin/make"),
        (
            5300,
            900,
            501,
            "cargo",
            f"{USER}/app.slice/x.service",
            "/home/m/.cargo/cargo",
        ),
        (5400, 1, 501, "python3", "0::/", "/usr/bin/python3.12"),
        # A rootless container's non-root user runs as a subordinate id.
        (
            5500,
            950,
            524288 + 1000,
            "postgres",
            f"{USER}/user.slice/nerdctl-1.scope",
            "/usr/lib/postgresql/bin/postgres",
        ),
        (
            5600,
            950,
            501,
            "containerd-shim",
            f"{USER}/app.slice/containerd.service",
            "/usr/local/bin/containerd-shim-runc-v2",
        ),
        # Root daemons and another user's subordinate ids are not work.
        (
            5700,
            1,
            0,
            "rsyslogd",
            "0::/system.slice/rsyslog.service",
            "/usr/sbin/rsyslogd",
        ),
        (5800, 1, 100500, "other", "0::/system.slice/x.service", "/usr/bin/other"),
        # A-8: a name proves nothing. Programs called like daemons, and
        # anything that re-executes /proc/self/exe, are busy unless their
        # executable is a declared one.
        (5900, 3901, 501, "exe", SESSION_CGROUP, "/home/m/build/exe"),
        (5901, 3950, 501, "containerd", SESSION_CGROUP, "/home/m/go/bin/containerd"),
        (5902, 900, 501, "systemd", f"{USER}/app.slice/y.service", "/tmp/systemd"),
        (5903, 1, 501, "sshd", "0::/", "/home/m/sshd"),
        # An executable no one can read is not idle.
        (5904, 3901, 501, "sshd-session", SESSION_CGROUP, "-"),
        # Another SSH session's processes run declared executables: idle,
        # also when apt replaced the binary under a running sshd.
        (6000, 1, 0, "sshd-session", SESSION_CGROUP, f"{SSHD_SESSION} (deleted)"),
        (6001, 6000, 501, "sshd-session", SESSION_CGROUP, SSHD_SESSION),
        # Its shell is work, like any process of the user.
        (6002, 6001, 501, "bash", SESSION_CGROUP, shell),
    ]
    assert {
        p.pid for p in linux_vm.busy_guest_processes(_probe(busy), definitions)
    } == {
        5000,
        5100,
        5200,
        5201,
        5300,
        5400,
        5500,
        5600,
        5900,
        5901,
        5902,
        5903,
        5904,
        6002,
    }
    with pytest.raises(linux_vm.LimaError, match="own shell"):
        linux_vm.busy_guest_processes(
            "501\nmolt\n4242\n--- subuid\n--- exe\n--- ps\n1 0 0 systemd -\n",
            definitions,
        )
    with pytest.raises(linux_vm.LimaError, match="unreadable"):
        linux_vm.busy_guest_processes("501\n4000\n", definitions)
    # A probe without the executable listing (an older probe) is unreadable.
    with pytest.raises(linux_vm.LimaError, match="unreadable"):
        linux_vm.busy_guest_processes(
            "501\nmolt\n4000\n--- subuid\n--- ps\n4000 1 501 sh -\n", definitions
        )


def apply_setup(
    definitions: linux_vm.Definitions, tmp_path: Path, **host_options: object
) -> tuple[FakeHost, FakeVm, str]:
    fake = matching_vm(definitions, "molt-linux", tmp_path / "lima")
    fake.config["mounts"] = [mounted(tmp_path)]
    fake.config["portForwards"] = None
    fake.processes = list(SESSION)
    rendered = linux_vm.lima_config(
        definitions.vms["molt-linux"],
        minimum_lima_version=definitions.minimum_lima_version,
    )
    text = linux_vm.render(
        definitions.vms["molt-linux"],
        minimum_lima_version=definitions.minimum_lima_version,
    )
    host = FakeHost([fake], **host_options)
    host.configs_by_text[text] = lima_filled(rendered)
    original = (fake.directory / "lima.yaml").read_text(encoding="utf-8")
    return host, fake, original


def run_apply(definitions, host, *, confirm: bool = True) -> int:
    return linux_vm.apply(
        "molt-linux", definitions=definitions, host=host, confirm_restart=confirm
    )


def test_apply_refuses_without_confirmation_and_runs_nothing(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, original = apply_setup(definitions, tmp_path)
    assert run_apply(definitions, host, confirm=False) == 1
    assert host.calls == []
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original


def test_apply_refuses_a_busy_guest_and_leaves_the_vm_running(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    host, fake, original = apply_setup(definitions, tmp_path)
    fake.processes.append((6000, 3901, 0, "sudo", SESSION_CGROUP, "/usr/bin/sudo"))
    assert run_apply(definitions, host) == 1
    assert "busy: sudo (pid 6000, uid 0)" in capsys.readouterr().out
    assert fake.status == "Running"
    assert not [call for call in host.calls if call[1] in ("stop", "start")]
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original
    assert not (fake.directory / linux_vm.NEXT_CONFIG).exists()


def test_apply_refuses_a_config_lima_rejects_before_touching_the_guest(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, original = apply_setup(definitions, tmp_path, validate_rc=1)
    assert run_apply(definitions, host) == 1
    assert [call[1] for call in host.calls] == ["list", "validate"]
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original
    assert not (fake.directory / linux_vm.NEXT_CONFIG).exists()


def test_apply_refuses_a_change_only_a_new_vm_can_make(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, _original = apply_setup(definitions, tmp_path)
    fake.vm_type = "qemu"
    assert run_apply(definitions, host) == 1
    assert [call[1] for call in host.calls] == ["list"]


def test_apply_refuses_when_the_guest_cannot_list_every_executable(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Without passwordless sudo the probe cannot read root's executables."""
    host, fake, original = apply_setup(definitions, tmp_path)
    fake.probe_rc = 3
    assert run_apply(definitions, host) == 1
    assert "cannot list molt-linux's processes: sudo" in capsys.readouterr().out
    assert [call[1] for call in host.calls] == ["list", "validate", "shell"]
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original


def test_apply_installs_the_definition_into_an_idle_vm_and_restarts_it(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, original = apply_setup(definitions, tmp_path)
    assert run_apply(definitions, host) == 0
    verbs = [call[1] for call in host.calls]
    assert verbs == ["list", "validate", "shell", "stop", "start", "list"]
    # HF-471: the start is a service launch; the fake's guarded `run` has no
    # `limactl start` at all.
    assert host.services == [("limactl", "start", "--tty=false", "molt-linux")]
    assert host.validated == [
        (fake.directory / "lima.yaml").read_text(encoding="utf-8")
    ]
    assert host.validated[0] in host.configs_by_text
    assert (fake.directory / linux_vm.PREVIOUS_CONFIG).read_text(
        encoding="utf-8"
    ) == original
    assert not (fake.directory / linux_vm.NEXT_CONFIG).exists()
    assert fake.status == "Running"
    assert linux_vm.compare(definitions.vms["molt-linux"], instance_of(fake)) == ()
    # The applied VM runs its definition: retirement now reads files only.
    assert linux_vm.runs_its_definition(fake.directory, definitions, tmp_path / "lima")


def test_apply_to_a_stopped_vm_installs_the_config_without_starting_it(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, original = apply_setup(definitions, tmp_path)
    fake.status = "Stopped"
    assert run_apply(definitions, host) == 0
    assert [call[1] for call in host.calls] == ["list", "validate", "list"]
    assert fake.status == "Stopped"
    assert (fake.directory / linux_vm.PREVIOUS_CONFIG).read_text(
        encoding="utf-8"
    ) == original


def test_a_failed_start_restores_and_restarts_the_replaced_config(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """B4: the VM comes back on the config it ran, and the original survives."""
    host, fake, original = apply_setup(definitions, tmp_path)
    host.start_outcomes = [(1, "Stopped"), (0, "Running")]
    assert run_apply(definitions, host) == 1
    out = capsys.readouterr().out
    assert "boot failed" in out and "restarted it with the replaced config" in out
    rendered = host.validated[0]
    assert host.started_with == [rendered, original]
    assert len(host.services) == 2, "the rollback start is a service launch too"
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original
    assert fake.status == "Running"
    assert (fake.directory / linux_vm.PREVIOUS_CONFIG).read_text(
        encoding="utf-8"
    ) == original
    # A retry fails the same way and still keeps the hand-made original.
    host.start_outcomes = [(1, "Stopped"), (1, "Stopped")]
    assert run_apply(definitions, host) == 1
    assert "does not run with that config either" in capsys.readouterr().out
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original
    assert (fake.directory / linux_vm.PREVIOUS_CONFIG).read_text(
        encoding="utf-8"
    ) == original


def test_a_vm_that_stops_after_its_start_fails_apply(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """HF-471: `limactl start` exited 0 and the hostagent died in that second.

    The exit status alone said the VM started; Lima's listing says it is
    stopped. apply fails, restores the replaced config, restarts the VM and
    checks the listing again; when that VM stops too, it says so.
    """
    host, fake, original = apply_setup(definitions, tmp_path)
    host.start_outcomes = [(0, "Stopped"), (0, "Running")]
    assert run_apply(definitions, host) == 1
    out = capsys.readouterr().out
    assert (
        "limactl start molt-linux exited 0, but Lima lists molt-linux as Stopped" in out
    )
    assert "restarted it with the replaced config" in out
    assert "OK:" not in out
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original
    assert fake.status == "Running"
    host.start_outcomes = [(0, "Stopped"), (0, "Broken")]
    assert run_apply(definitions, host) == 1
    out = capsys.readouterr().out
    assert "does not run with that config either" in out
    assert "but Lima lists molt-linux as Broken" in out
    assert (fake.directory / "lima.yaml").read_text(encoding="utf-8") == original


def test_a_start_that_fails_on_a_running_vm_is_reported_not_rolled_back(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """limactl failed (a timeout, a degraded requirement) but the VM runs.

    A rollback would need another stop; apply reports the failure and the
    config the VM runs.
    """
    host, fake, _original = apply_setup(definitions, tmp_path)
    host.start_outcomes = [(1, "Running")]
    assert run_apply(definitions, host) == 1
    out = capsys.readouterr().out
    assert "exited 1: INFO starting | vz: boot failed" in out
    assert "molt-linux runs the installed definition" in out
    assert len(host.started_with) == 1
    installed = (fake.directory / "lima.yaml").read_text(encoding="utf-8")
    assert installed == host.validated[0]


def test_apply_removes_the_forget_sets_of_the_mounts_it_ends(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """A-5: a release's forget set lives as long as its mount.

    The applied VM mounts nothing, so the set under its old mount goes; a
    set under a location another VM still mounts stays, and so does a set
    directory holding anything the release did not create.
    """
    host, fake, _original = apply_setup(definitions, tmp_path)
    other, deep, kept = tmp_path / "other", tmp_path / "deep", tmp_path / "kept"
    nested = deep / "nested"
    for location in (other, nested, kept):
        location.mkdir(parents=True)
        fake.config["mounts"].append(mounted(location))  # type: ignore[union-attr]
    for location in (tmp_path, other, nested, kept):
        linux_vm.forget_set(location)
    (kept / linux_vm.FORGET_SET / "notes.txt").write_text("x", encoding="utf-8")
    # A stopped VM still mounts `other`, and `deep`, the parent of `nested`.
    x86 = matching_vm(definitions, "molt-x86", tmp_path / "lima", status="Stopped")
    x86.config["mounts"] = [mounted(other), mounted(deep)]
    host.vms["molt-x86"] = x86
    # A release record of the mounting VM ends with its mounts too.
    (fake.directory / linux_vm.RELEASE_RECORD).write_text("{}", encoding="utf-8")
    assert run_apply(definitions, host) == 0
    out = capsys.readouterr().out
    assert not (fake.directory / linux_vm.RELEASE_RECORD).exists()
    assert not (tmp_path / linux_vm.FORGET_SET).exists()
    assert f"removed {tmp_path / linux_vm.FORGET_SET}" in out
    for location in (other, nested):
        assert len(os.listdir(location / linux_vm.FORGET_SET)) == linux_vm.FORGET_BATCH
    assert os.listdir(kept / linux_vm.FORGET_SET) == ["notes.txt"]
    assert f"kept {kept / linux_vm.FORGET_SET}" in out


def test_a_second_apply_keeps_the_first_replaced_config(
    definitions: linux_vm.Definitions, tmp_path: Path
) -> None:
    host, fake, original = apply_setup(definitions, tmp_path)
    assert run_apply(definitions, host) == 0
    assert run_apply(definitions, host) == 0
    assert (fake.directory / linux_vm.PREVIOUS_CONFIG).read_text(
        encoding="utf-8"
    ) == original


# --------------------------------------------------------------------------
# Service launch (the real host)
# --------------------------------------------------------------------------


def test_a_service_launch_leaves_its_service_running_in_its_own_group(
    tmp_path: Path,
) -> None:
    """HF-471: the real `LocalHost.start_service`; the kernel is the oracle.

    The command starts a background loop (the service) that inherits the
    command's output, prints the loop's pid, its own pid and its process
    group, and exits. The call returns when the command exits, though the
    service still holds the output; the service is alive afterwards (Molt's
    guard would have closed it before returning); and the command led its
    own process group, apart from this test's.
    """
    stop = tmp_path / "stop"
    loop = f"while [ ! -e {shlex.quote(str(stop))} ]; do sleep 0.05; done; echo late"
    script = f'({loop}) & echo "$! $$ $(ps -o pgid= -p $$)"'
    service = None
    try:
        completed = linux_vm.LocalHost(os.environ).start_service(
            ["sh", "-c", script], timeout=60.0
        )
        assert completed.returncode == 0, completed.stderr
        service, shell, group = (int(word) for word in completed.stdout.split())
        assert pid_alive(service), "the service outlives the command"
        assert group == shell != os.getpgid(0), "the command leads its own group"
    finally:
        stop.touch()
    deadline = time.monotonic() + 10.0
    while service is not None and pid_alive(service) and time.monotonic() < deadline:
        time.sleep(0.05)
    assert service is None or not pid_alive(service)


def test_a_service_launch_ends_only_its_command_at_the_deadline() -> None:
    host = linux_vm.LocalHost(os.environ)
    completed = host.start_service(["sh", "-c", "echo $$; exec sleep 30"], timeout=0.5)
    assert completed.returncode == 124
    assert completed.stderr.endswith("sh timed out after 0.5 s\n"), completed.stderr
    assert not pid_alive(int(completed.stdout.split()[0]))
    missing = host.start_service(["/nonexistent/limactl", "start"], timeout=5.0)
    assert missing.returncode == 127 and "cannot run" in missing.stderr


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def test_check_exits_one_on_drift_and_zero_when_every_vm_matches(
    definitions: linux_vm.Definitions,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    linux, x86, _custody = two_vms(definitions, tmp_path)
    x86.config.pop("mounts")
    x86.cached = []
    host = FakeHost([linux, x86])
    assert linux_vm.main(["check", "--json"], host=host) == 1
    report = json.loads(capsys.readouterr().out)
    assert report["schema"] == linux_vm.CHECK_SCHEMA
    rows = {row["name"]: row for row in report["vms"]}
    assert [d["field"] for d in rows["molt-linux"]["drift"]] == ["mounts"]
    assert rows["molt-x86"]["drift"] == []
    assert rows["molt-linux"]["held"]["deleted_files"] == 3
    # A VM that mounts nothing pins nothing: no held-file report.
    assert rows["molt-x86"]["held"] is None
    assert not [call for call in host.calls if call[1:2] == ("shell",)], (
        "check runs nothing in a guest"
    )
    linux.config.pop("mounts")
    host.calls.clear()
    assert linux_vm.main(["check"], host=host) == 0
    assert [call[0] for call in host.calls] == ["limactl"], "no lsof without mounts"


def test_check_without_lima_has_nothing_to_check() -> None:
    host = FakeHost(lima=False)
    assert linux_vm.main(["check"], host=host) == 0
    assert host.calls == []


# --------------------------------------------------------------------------
# The real guests (opt-in)
# --------------------------------------------------------------------------

REAL_VMS = os.environ.get("MOLT_LINUX_VM_TESTS") == "1"


@pytest.mark.skipif(
    not REAL_VMS,
    reason=(
        "acts on the developer's running Lima VMs (drops their caches, starts a "
        "short guest reader); set MOLT_LINUX_VM_TESTS=1 to run it"
    ),
)
def test_release_frees_what_a_real_guest_pinned() -> None:
    """The oracle is the real VM process's descriptors, read by lsof.

    For each running VM that mounts a host path: write probe files under the
    mount, have the guest read, stat, list and mmap them, keep one open in a
    guest process, delete them on the host, and release. Every deleted probe
    file is freed but the open one; once its reader exits, a release frees it.
    """
    environment = {**os.environ, "LIMA_HOME": str(Path.home() / ".lima")}
    host = linux_vm.LocalHost(environment)
    if host.which("limactl") is None:
        pytest.skip("Lima is not installed")
    instances = linux_vm._pinning(linux_vm.list_instances(host))
    if not instances:
        pytest.skip("no running Lima VM mounts a host path (all applied)")
    for instance in instances:
        location, point, _writable = instance.mounts()[0]
        name = f".molt-linux-vm-test-{uuid.uuid4().hex[:12]}"
        probe, guest = location / name, point / name
        probe.mkdir()
        kinds = ("read", "stat", "listed", "mmap", "open")
        try:
            for kind in kinds:
                (probe / f"{kind}.bin").write_bytes(os.urandom(4096))
            g = shlex.quote(str(guest))
            reader = host.run(
                [
                    "limactl",
                    "shell",
                    "--workdir",
                    "/",
                    instance.name,
                    "sh",
                    "-c",
                    f"cat {g}/read.bin >/dev/null && stat {g}/stat.bin >/dev/null"
                    f" && ls {g} >/dev/null && python3 -c 'import mmap,sys;"
                    f' f=open(sys.argv[1],"rb");'
                    f" mmap.mmap(f.fileno(),0,prot=mmap.PROT_READ)[:1]' {g}/mmap.bin"
                    f" && (nohup setsid sh -c 'exec 3< {g}/open.bin; sleep 8'"
                    " >/dev/null 2>&1 &) && sleep 1",
                ],
                timeout=60.0,
            )
            assert reader.returncode == 0, reader.stderr
        finally:
            for kind in kinds:
                (probe / f"{kind}.bin").unlink(missing_ok=True)
            probe.rmdir()

        def held_probes() -> set[str]:
            listed = host.run(
                ["lsof", "-n", "-P", "-a", "+L1", "-p", str(pid), "-F", "n"],
                timeout=60.0,
            )
            return {
                Path(line[1:]).name
                for line in listed.stdout.splitlines()
                if line.startswith(f"n{probe}/")
            }

        pid = linux_vm.held_files(host, [instance])[instance.name].pid
        assert held_probes() == {f"{kind}.bin" for kind in kinds}, (
            "the deleted probe files are pinned before the release"
        )
        [result] = linux_vm.release(host, [instance])
        assert result.error is None, result.error
        assert held_probes() == {"open.bin"}, "the release frees all but the open file"
        time.sleep(9)
        [result] = linux_vm.release(host, [instance])
        assert result.error is None, result.error
        assert held_probes() == set(), "a release after the reader exits frees it"


_LIMA_USER_UNITS = (
    "/init.scope",
    "/app.slice/containerd.service",
    "/app.slice/default-buildkit.service",
    "/app.slice/stargz-snapshotter.service",
)


@pytest.mark.skipif(
    not REAL_VMS,
    reason=(
        "reads the developer's running Lima VMs' processes (as root, with "
        "`sudo ps`); set MOLT_LINUX_VM_TESTS=1 to run it"
    ),
)
def test_the_real_guests_daemons_are_idle_by_their_executables(
    definitions: linux_vm.Definitions,
) -> None:
    """A-8 against the real guests; it starts, stops and writes nothing.

    In each running defined VM the probe lists every process with its
    executable. The processes of the units Lima provisions for the guest user
    (its systemd, rootless containerd, BuildKit, stargz) run declared
    executables, so none is busy; a containerd shim would be.
    """
    host = linux_vm.LocalHost({**os.environ, "LIMA_HOME": str(Path.home() / ".lima")})
    if host.which("limactl") is None:
        pytest.skip("Lima is not installed")
    running = [
        instance
        for instance in linux_vm.list_instances(host)
        if instance.running and instance.name in definitions.vms
    ]
    if not running:
        pytest.skip("no defined Lima VM is running")
    for instance in running:
        probe = host.run(
            [
                "limactl",
                "shell",
                "--workdir",
                "/",
                instance.name,
                "sh",
                "-c",
                linux_vm.IDLE_PROBE,
            ],
            timeout=linux_vm.GUEST_COMMAND_SECONDS,
        )
        assert probe.returncode == 0, probe.stderr
        parsed = linux_vm._parse_probe(probe.stdout)
        daemons = {
            process.pid
            for process in parsed.processes.values()
            if process.uid == parsed.guest_uid
            and "/user@" in process.cgroup
            and process.cgroup.endswith(_LIMA_USER_UNITS)
            and not process.comm.startswith("containerd-shim")
        }
        assert daemons, f"{instance.name} runs no user daemon"
        busy = {
            process.pid
            for process in linux_vm.busy_guest_processes(probe.stdout, definitions)
        }
        assert not busy & daemons, [
            (parsed.processes[pid], parsed.executables.get(pid))
            for pid in sorted(busy & daemons)
        ]
