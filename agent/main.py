from __future__ import annotations

import logging
from contextlib import asynccontextmanager

from fastapi import Depends, FastAPI, Header, HTTPException, status
from fastapi.responses import JSONResponse

from .config import Settings, get_settings
from .execution import CommandPolicy, E2BBackend, LocalDryRunBackend
from .model_client import ModelClient
from .models import Event, RunRequest, RunResponse, RunState
from .runtime import AgentRuntime
from .storage import Storage


def build_runtime(settings: Settings) -> AgentRuntime:
    storage = Storage(settings.database_url)
    if settings.e2b_api_key:
        backend = E2BBackend(
            template=settings.e2b_template,
            timeout_seconds=settings.e2b_timeout_seconds,
            command_timeout=settings.max_command_seconds,
            max_output_bytes=settings.max_output_bytes,
        )
    else:
        backend = LocalDryRunBackend()
    model = ModelClient(settings.model_base_url, settings.model_api_key, settings.model_name)
    runtime = AgentRuntime(storage, backend, model, CommandPolicy())
    runtime.start()
    return runtime


@asynccontextmanager
async def lifespan(app: FastAPI):
    settings = get_settings()
    logging.basicConfig(level=settings.log_level.upper())
    app.state.settings = settings
    app.state.runtime = build_runtime(settings)
    yield


app = FastAPI(title="Hermes Agent Alternative", version="0.1.0", lifespan=lifespan)


def get_runtime() -> AgentRuntime:
    runtime = getattr(app.state, "runtime", None)
    if runtime is None:
        raise HTTPException(status_code=503, detail="Runtime is starting")
    return runtime


def require_auth(authorization: str | None = Header(default=None)) -> None:
    settings = get_settings()
    if not settings.api_token:
        return
    expected = f"Bearer {settings.api_token}"
    if authorization != expected:
        raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="Unauthorized")


@app.get("/healthz")
async def healthz() -> JSONResponse:
    return JSONResponse({"status": "ok", "service": "hermes-agent-alternative"})


@app.post("/v1/runs", response_model=RunResponse, dependencies=[Depends(require_auth)])
async def create_run(request: RunRequest, runtime: AgentRuntime = Depends(get_runtime)) -> RunResponse:
    state = await runtime.submit(request)
    return RunResponse(run_id=state.run_id, status=state.status)


@app.get("/v1/runs/{run_id}", response_model=RunState, dependencies=[Depends(require_auth)])
async def get_run(run_id: str, runtime: AgentRuntime = Depends(get_runtime)) -> RunState:
    state = await runtime.wait(run_id)
    if state is None:
        raise HTTPException(status_code=404, detail="Run not found")
    return state


@app.get("/v1/runs/{run_id}/events", response_model=list[Event], dependencies=[Depends(require_auth)])
async def get_events(run_id: str, runtime: AgentRuntime = Depends(get_runtime)) -> list[Event]:
    if runtime.storage.get_run(run_id) is None:
        raise HTTPException(status_code=404, detail="Run not found")
    return runtime.storage.list_events(run_id)


def run() -> None:
    import uvicorn

    settings = get_settings()
    uvicorn.run("agent.main:app", host=settings.host, port=settings.port, reload=False)


if __name__ == "__main__":
    run()
