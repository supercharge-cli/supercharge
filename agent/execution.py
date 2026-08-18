from __future__ import annotations

import asyncio
import shlex
from dataclasses import dataclass
from typing import Protocol


@dataclass(slots=True)
class CommandResult:
    stdout: str
    stderr: str
    exit_code: int


class ExecutionBackend(Protocol):
    async def run(self, command: str, *, workspace_id: str | None = None) -> CommandResult: ...


class CommandPolicy:
    """Small baseline policy; production deployments should add an approval service."""

    _blocked_tokens = {
        "mkfs",
        "rm",
        "mount",
        "umount",
        "shutdown",
        "reboot",
        "poweroff",
        "systemctl",
        "iptables",
        "nft",
        "curl",
        "wget",
    }

    def validate(self, command: str) -> None:
        if not command.strip():
            raise ValueError("Command cannot be empty")
        tokens = {token.lower() for token in shlex.split(command)}
        blocked = tokens & self._blocked_tokens
        if blocked:
            raise PermissionError(f"Command blocked by policy: {', '.join(sorted(blocked))}")
        if ".." in command and ("/" in command or "\\" in command):
            raise PermissionError("Path traversal is not allowed")


class LocalDryRunBackend:
    async def run(self, command: str, *, workspace_id: str | None = None) -> CommandResult:
        await asyncio.sleep(0)
        return CommandResult(
            stdout=f"dry-run: {command}",
            stderr="",
            exit_code=0,
        )


class E2BBackend:
    def __init__(self, template: str, timeout_seconds: int, command_timeout: int, max_output_bytes: int) -> None:
        self.template = template
        self.timeout_seconds = timeout_seconds
        self.command_timeout = command_timeout
        self.max_output_bytes = max_output_bytes

    async def run(self, command: str, *, workspace_id: str | None = None) -> CommandResult:
        try:
            from e2b import AsyncSandbox
        except ImportError as exc:
            raise RuntimeError("Install the optional E2B dependency with: pip install -e '.[e2b]'") from exc

        sandbox = None
        try:
            if workspace_id:
                sandbox = await AsyncSandbox.connect(workspace_id)
                await sandbox.set_timeout(timeout=self.timeout_seconds)
            else:
                sandbox = await AsyncSandbox.create(
                    template=self.template,
                    timeout=self.timeout_seconds,
                    metadata={"service": "hermes-agent-alternative"},
                )
            result = await sandbox.commands.run(command, timeout=self.command_timeout)
            stdout = (result.stdout or "")[: self.max_output_bytes]
            stderr = (result.stderr or "")[: self.max_output_bytes]
            return CommandResult(stdout=stdout, stderr=stderr, exit_code=result.exit_code or 0)
        finally:
            if sandbox is not None:
                try:
                    await sandbox.kill()
                except Exception:
                    # The sandbox may have expired or already been killed; do not mask the run result.
                    pass
