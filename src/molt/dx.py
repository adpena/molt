from __future__ import annotations

import os
import shlex
import subprocess
import tomllib
from pathlib import Path
from typing import Mapping, cast


TEST_PYTHONS = ["3.12", "3.13", "3.14"]


class DxConfigError(RuntimeError):
    pass


class DxProject:
    def __init__(self, root: Path) -> None:
        self.root = root.resolve()

    @classmethod
    def from_current_repo(cls) -> "DxProject":
        return cls(Path(__file__).resolve().parents[2])

    def load_config(self) -> dict[str, object]:
        with (self.root / "pyproject.toml").open("rb") as fh:
            data = tomllib.load(fh)
        tool = data.get("tool", {})
        if not isinstance(tool, dict):
            return {}
        molt = tool.get("molt", {})
        if not isinstance(molt, dict):
            return {}
        dx = molt.get("dx", {})
        return dx if isinstance(dx, dict) else {}

    def commands(self) -> dict[str, object]:
        commands = self.load_config().get("commands", {})
        return cast(dict[str, object], commands) if isinstance(commands, dict) else {}

    def project_env_dir(self) -> Path:
        return self.root / ".venv"

    def project_python(self) -> Path:
        if os.name == "nt":
            return self.project_env_dir() / "Scripts" / "python.exe"
        return self.project_env_dir() / "bin" / "python3"

    def project_env_matches_python(self, requested: str | None) -> bool:
        project_python = self.project_python()
        if not project_python.exists():
            return False
        if not requested:
            return True
        try:
            proc = subprocess.run(
                [
                    str(project_python),
                    "-c",
                    "import sys; print(f'{sys.version_info[0]}.{sys.version_info[1]}')",
                ],
                cwd=self.root,
                capture_output=True,
                text=True,
                check=True,
            )
        except (OSError, subprocess.CalledProcessError):
            return False
        return proc.stdout.strip() == requested

    def normalized_uv_run_env(
        self,
        env: Mapping[str, str],
        *,
        python: str | None,
    ) -> dict[str, str]:
        run_env = dict(env)
        run_env.setdefault("PYTHONUNBUFFERED", "1")
        run_env["UV_PROJECT_ENVIRONMENT"] = str(self.project_env_dir())
        for name in ("VIRTUAL_ENV", "PYTHONHOME", "CONDA_PREFIX", "CONDA_DEFAULT_ENV"):
            run_env.pop(name, None)
        if run_env.get("UV_NO_SYNC") == "1" and not self.project_env_matches_python(
            python
        ):
            run_env.pop("UV_NO_SYNC", None)
        return run_env

    def canonical_env(
        self,
        base: Mapping[str, str] | None = None,
        *,
        create_dirs: bool = True,
    ) -> dict[str, str]:
        dx = self.load_config()
        env = dict(os.environ if base is None else base)
        for name in ("VIRTUAL_ENV", "PYTHONHOME", "CONDA_PREFIX", "CONDA_DEFAULT_ENV"):
            env.pop(name, None)
        env_cfg = dx.get("env", {})
        if isinstance(env_cfg, dict):
            for key, raw_value in env_cfg.items():
                if not isinstance(key, str) or not isinstance(raw_value, str):
                    continue
                env[key] = raw_value.format(root=str(self.root))
        env.setdefault("MOLT_SESSION_ID", f"dev-{os.getpid()}")
        env.setdefault("MOLT_BACKEND_DAEMON", "1" if dx.get("backend_daemon") else "0")
        env.setdefault("CARGO_BUILD_JOBS", str(dx.get("cargo_build_jobs", 2)))
        if create_dirs:
            for key in (
                "CARGO_TARGET_DIR",
                "MOLT_CACHE",
                "MOLT_DIFF_ROOT",
                "MOLT_DIFF_TMPDIR",
                "UV_CACHE_DIR",
                "TMPDIR",
            ):
                value = env.get(key)
                if value:
                    Path(value).mkdir(parents=True, exist_ok=True)
        return env

    def require_project_python(self, context: str) -> Path:
        python = self.project_python()
        if not python.exists():
            raise DxConfigError(
                f"{python} is missing; run `tools/dev.py install` before {context}"
            )
        return python

    def format_command(self, command: str) -> str:
        return command.format(
            root=str(self.root), project_python=str(self.project_python())
        )

    def split_command(self, command: object, name: str) -> list[str]:
        if not isinstance(command, str) or not command.strip():
            raise DxConfigError(f"Missing [tool.molt.dx.commands].{name}")
        return shlex.split(self.format_command(command), posix=os.name != "nt")

    def split_command_sequence(
        self,
        command: object,
        name: str,
        *,
        commands: dict[str, object] | None = None,
        stack: tuple[str, ...] = (),
    ) -> list[list[str]]:
        commands = self.commands() if commands is None else commands

        def split_item(item: str, item_name: str) -> list[list[str]]:
            stripped = item.strip()
            if stripped.startswith("@"):
                ref = stripped[1:]
                if not ref or any(ch.isspace() for ch in ref):
                    raise DxConfigError(
                        f"Invalid [tool.molt.dx.commands].{item_name} reference: {item!r}"
                    )
                if ref in stack:
                    chain = " -> ".join((*stack, ref))
                    raise DxConfigError(
                        f"Cyclic [tool.molt.dx.commands] reference: {chain}"
                    )
                if ref not in commands:
                    raise DxConfigError(
                        f"Missing [tool.molt.dx.commands].{ref} referenced by {item_name}"
                    )
                return self.split_command_sequence(
                    commands[ref],
                    ref,
                    commands=commands,
                    stack=(*stack, ref),
                )
            return [self.split_command(item, item_name)]

        if isinstance(command, str):
            return split_item(command, name)
        if isinstance(command, list) and command:
            split: list[list[str]] = []
            for idx, item in enumerate(command):
                if not isinstance(item, str) or not item.strip():
                    raise DxConfigError(
                        f"Invalid [tool.molt.dx.commands].{name}[{idx}]: "
                        "expected command string"
                    )
                split.extend(split_item(item, f"{name}[{idx}]"))
            return split
        raise DxConfigError(f"Missing [tool.molt.dx.commands].{name}")
