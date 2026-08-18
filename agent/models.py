from datetime import datetime, timezone
from enum import StrEnum
from typing import Any
from uuid import uuid4

from pydantic import BaseModel, Field


def utc_now() -> datetime:
    return datetime.now(timezone.utc)


class RunStatus(StrEnum):
    QUEUED = "queued"
    RUNNING = "running"
    SUCCEEDED = "succeeded"
    FAILED = "failed"
    CANCELLED = "cancelled"


class RunRequest(BaseModel):
    prompt: str = Field(min_length=1, max_length=20_000)
    workspace_id: str | None = Field(default=None, max_length=128)
    command: str | None = Field(default=None, max_length=4_000)
    wait: bool = False


class RunResponse(BaseModel):
    run_id: str
    status: RunStatus


class RunState(BaseModel):
    run_id: str = Field(default_factory=lambda: str(uuid4()))
    prompt: str
    workspace_id: str | None = None
    command: str | None = None
    status: RunStatus = RunStatus.QUEUED
    result: str | None = None
    error: str | None = None
    created_at: datetime = Field(default_factory=utc_now)
    updated_at: datetime = Field(default_factory=utc_now)


class Event(BaseModel):
    run_id: str
    kind: str
    payload: dict[str, Any] = Field(default_factory=dict)
    created_at: datetime = Field(default_factory=utc_now)
