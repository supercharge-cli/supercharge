from __future__ import annotations

import asyncio
import logging
from collections.abc import Awaitable, Callable

from .execution import CommandPolicy, ExecutionBackend
from .model_client import ModelClient
from .models import Event, RunRequest, RunState, RunStatus
from .storage import Storage

logger = logging.getLogger(__name__)


class AgentRuntime:
    def __init__(
        self,
        storage: Storage,
        backend: ExecutionBackend,
        model: ModelClient,
        policy: CommandPolicy,
    ) -> None:
        self.storage = storage
        self.backend = backend
        self.model = model
        self.policy = policy
        self._tasks: dict[str, asyncio.Task[None]] = {}
        self._listeners: set[Callable[[Event], Awaitable[None]]] = set()

    def start(self) -> None:
        interrupted = self.storage.recover_interrupted_runs()
        if interrupted:
            logger.warning("Marked %s interrupted runs as failed", interrupted)

    def add_listener(self, listener: Callable[[Event], Awaitable[None]]) -> None:
        self._listeners.add(listener)

    async def submit(self, request: RunRequest) -> RunState:
        state = RunState(
            prompt=request.prompt,
            workspace_id=request.workspace_id,
            command=request.command,
        )
        self.storage.create_run(state)
        await self._emit(Event(run_id=state.run_id, kind="run.queued", payload={"prompt": request.prompt}))
        task = asyncio.create_task(self._execute(state), name=f"run-{state.run_id}")
        self._tasks[state.run_id] = task
        task.add_done_callback(lambda _: self._tasks.pop(state.run_id, None))
        if request.wait:
            await task
            return self.storage.get_run(state.run_id) or state
        return state

    async def wait(self, run_id: str) -> RunState | None:
        task = self._tasks.get(run_id)
        if task:
            await task
        return self.storage.get_run(run_id)

    async def _execute(self, state: RunState) -> None:
        try:
            state.status = RunStatus.RUNNING
            self.storage.update_run(state)
            await self._emit(Event(run_id=state.run_id, kind="run.started"))

            tool_output = None
            if state.command:
                self.policy.validate(state.command)
                await self._emit(
                    Event(run_id=state.run_id, kind="tool.started", payload={"command": state.command})
                )
                result = await self.backend.run(state.command, workspace_id=state.workspace_id)
                tool_output = f"exit_code={result.exit_code}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
                await self._emit(
                    Event(
                        run_id=state.run_id,
                        kind="tool.finished",
                        payload={"exit_code": result.exit_code, "stdout": result.stdout, "stderr": result.stderr},
                    )
                )
                if result.exit_code != 0:
                    raise RuntimeError(f"Command failed with exit code {result.exit_code}")

            state.result = await self.model.complete(state.prompt, tool_output=tool_output)
            state.status = RunStatus.SUCCEEDED
            self.storage.update_run(state)
            await self._emit(Event(run_id=state.run_id, kind="run.succeeded"))
        except asyncio.CancelledError:
            state.status = RunStatus.CANCELLED
            state.error = "Run cancelled"
            self.storage.update_run(state)
            await self._emit(Event(run_id=state.run_id, kind="run.cancelled"))
            raise
        except Exception as exc:  # noqa: BLE001 - failure must be persisted and surfaced
            logger.exception("Run %s failed", state.run_id)
            state.status = RunStatus.FAILED
            state.error = str(exc)
            self.storage.update_run(state)
            await self._emit(Event(run_id=state.run_id, kind="run.failed", payload={"error": str(exc)}))

    async def _emit(self, event: Event) -> None:
        self.storage.append_event(event)
        for listener in tuple(self._listeners):
            try:
                await listener(event)
            except Exception:  # noqa: BLE001 - observability must not break execution
                logger.exception("Event listener failed for %s", event.run_id)
