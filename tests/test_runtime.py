from pathlib import Path

import pytest

from agent.execution import CommandPolicy, LocalDryRunBackend
from agent.model_client import ModelClient
from agent.models import RunRequest, RunStatus
from agent.runtime import AgentRuntime
from agent.storage import Storage


def make_runtime(tmp_path: Path) -> AgentRuntime:
    storage = Storage(f"sqlite:///{tmp_path / 'test.db'}")
    return AgentRuntime(
        storage=storage,
        backend=LocalDryRunBackend(),
        model=ModelClient("https://example.invalid/v1", "", "test-model"),
        policy=CommandPolicy(),
    )


def test_command_policy_blocks_dangerous_commands() -> None:
    with pytest.raises(PermissionError):
        CommandPolicy().validate("rm -rf /")


@pytest.mark.asyncio
async def test_runtime_runs_prompt_and_command(tmp_path: Path) -> None:
    runtime = make_runtime(tmp_path)
    state = await runtime.submit(
        RunRequest(prompt="List the project", command="printf hello", wait=True)
    )
    assert state.status == RunStatus.SUCCEEDED
    assert state.result is not None
    assert "dry-run: printf hello" in state.result
    events = runtime.storage.list_events(state.run_id)
    assert [event.kind for event in events] == [
        "run.queued",
        "run.started",
        "tool.started",
        "tool.finished",
        "run.succeeded",
    ]


@pytest.mark.asyncio
async def test_runtime_persists_failure(tmp_path: Path) -> None:
    runtime = make_runtime(tmp_path)
    state = await runtime.submit(RunRequest(prompt="blocked", command="curl https://example.com", wait=True))
    assert state.status == RunStatus.FAILED
    assert "blocked by policy" in (state.error or "")
