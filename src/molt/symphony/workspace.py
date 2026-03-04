from __future__ import annotations

import re
import shutil
import subprocess
from pathlib import Path

from .errors import HookError, WorkspaceError
from .logging_utils import log
from .models import Workspace, WorkspaceConfig, WorkspaceHooks


_SANITIZE_RE = re.compile(r"[^A-Za-z0-9._-]")


def sanitize_workspace_key(issue_identifier: str) -> str:
    cleaned = _SANITIZE_RE.sub("_", issue_identifier)
    if not cleaned:
        return "issue"
    return cleaned


class WorkspaceManager:
    def __init__(self, config: WorkspaceConfig, hooks: WorkspaceHooks) -> None:
        self._config = config
        self._hooks = hooks

    @property
    def root(self) -> Path:
        return self._config.root

    def create_for_issue(self, issue_identifier: str) -> Workspace:
        key = sanitize_workspace_key(issue_identifier)
        root = self._config.root
        root.mkdir(parents=True, exist_ok=True)
        path = root / key
        self._ensure_inside_root(path)

        created_now = False
        if path.exists() and not path.is_dir():
            raise WorkspaceError(f"workspace_path_exists_not_dir path={path}")
        if not path.exists():
            path.mkdir(parents=True, exist_ok=True)
            created_now = True

        workspace = Workspace(path=path, workspace_key=key, created_now=created_now)
        if created_now and self._hooks.after_create:
            self.run_hook(
                "after_create", self._hooks.after_create, workspace.path, fatal=True
            )

        return workspace

    def remove_workspace(self, issue_identifier: str) -> None:
        key = sanitize_workspace_key(issue_identifier)
        path = self._config.root / key
        self._ensure_inside_root(path)
        if not path.exists():
            return
        if self._hooks.before_remove:
            try:
                self.run_hook(
                    "before_remove",
                    self._hooks.before_remove,
                    path,
                    fatal=False,
                )
            except HookError:
                pass
        shutil.rmtree(path, ignore_errors=False)

    def run_before_run(self, workspace_path: Path) -> None:
        if self._hooks.before_run:
            self.run_hook(
                "before_run", self._hooks.before_run, workspace_path, fatal=True
            )

    def run_after_run(self, workspace_path: Path) -> None:
        if self._hooks.after_run:
            self.run_hook(
                "after_run", self._hooks.after_run, workspace_path, fatal=False
            )

    def run_hook(self, name: str, script: str, cwd: Path, fatal: bool) -> None:
        self._ensure_inside_root(cwd)
        timeout_sec = max(self._hooks.timeout_ms, 1) / 1000.0
        log("INFO", "workspace_hook_start", hook=name, cwd=cwd)
        try:
            result = subprocess.run(
                ["bash", "-lc", script],
                cwd=cwd,
                check=False,
                capture_output=True,
                text=True,
                timeout=timeout_sec,
            )
        except subprocess.TimeoutExpired as exc:
            message = f"hook_timeout hook={name} timeout_ms={self._hooks.timeout_ms}"
            if fatal:
                raise HookError(message) from exc
            log("WARNING", message, hook=name, cwd=cwd)
            return
        except OSError as exc:
            message = f"hook_os_error hook={name} error={exc}"
            if fatal:
                raise HookError(message) from exc
            log("WARNING", message, hook=name, cwd=cwd)
            return

        if result.returncode != 0:
            stderr = _truncate(result.stderr)
            message = f"hook_failed hook={name} rc={result.returncode} stderr={stderr}"
            if fatal:
                raise HookError(message)
            log(
                "WARNING",
                "workspace_hook_failed",
                hook=name,
                rc=result.returncode,
                stderr=stderr,
            )
            return

        stdout = _truncate(result.stdout)
        if stdout:
            log("INFO", "workspace_hook_output", hook=name, stdout=stdout)

    def ensure_workspace_cwd(self, workspace_path: Path) -> None:
        self._ensure_inside_root(workspace_path)

    def _ensure_inside_root(self, workspace_path: Path) -> None:
        root = self._config.root.resolve()
        candidate = workspace_path.resolve()
        if candidate == root:
            return
        if root not in candidate.parents:
            raise WorkspaceError(
                f"workspace_outside_root workspace_path={candidate} workspace_root={root}"
            )


def _truncate(value: str, max_chars: int = 1000) -> str:
    cleaned = value.strip()
    if len(cleaned) <= max_chars:
        return cleaned
    return cleaned[:max_chars] + "..."
