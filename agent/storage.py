import json
import sqlite3
from pathlib import Path
from threading import Lock

from .models import Event, RunState, RunStatus, utc_now


class Storage:
    def __init__(self, database_url: str) -> None:
        self._lock = Lock()
        self._path = self._parse_path(database_url)
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._initialize()

    @staticmethod
    def _parse_path(database_url: str) -> Path:
        prefix = "sqlite:///"
        if not database_url.startswith(prefix):
            raise ValueError("Only sqlite:/// database URLs are supported in v0.1")
        return Path(database_url.removeprefix(prefix)).expanduser()

    def _connect(self) -> sqlite3.Connection:
        connection = sqlite3.connect(self._path, check_same_thread=False)
        connection.row_factory = sqlite3.Row
        return connection

    def _initialize(self) -> None:
        with self._lock, self._connect() as connection:
            connection.executescript(
                """
                CREATE TABLE IF NOT EXISTS runs (
                    run_id TEXT PRIMARY KEY,
                    prompt TEXT NOT NULL,
                    workspace_id TEXT,
                    command TEXT,
                    status TEXT NOT NULL,
                    result TEXT,
                    error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    run_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_events_run_id ON events(run_id, id);
                """
            )
            columns = {row["name"] for row in connection.execute("PRAGMA table_info(runs)")}
            if "command" not in columns:
                connection.execute("ALTER TABLE runs ADD COLUMN command TEXT")

    def create_run(self, state: RunState) -> None:
        with self._lock, self._connect() as connection:
            connection.execute(
                """INSERT INTO runs
                (run_id, prompt, workspace_id, command, status, result, error, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    state.run_id,
                    state.prompt,
                    state.workspace_id,
                    state.command,
                    state.status.value,
                    state.result,
                    state.error,
                    state.created_at.isoformat(),
                    state.updated_at.isoformat(),
                ),
            )

    def update_run(self, state: RunState) -> None:
        state.updated_at = utc_now()
        with self._lock, self._connect() as connection:
            connection.execute(
                """UPDATE runs SET status=?, result=?, error=?, updated_at=? WHERE run_id=?""",
                (state.status.value, state.result, state.error, state.updated_at.isoformat(), state.run_id),
            )

    def get_run(self, run_id: str) -> RunState | None:
        with self._lock, self._connect() as connection:
            row = connection.execute("SELECT * FROM runs WHERE run_id=?", (run_id,)).fetchone()
        return self._state_from_row(row) if row else None

    def list_events(self, run_id: str) -> list[Event]:
        with self._lock, self._connect() as connection:
            rows = connection.execute(
                "SELECT run_id, kind, payload, created_at FROM events WHERE run_id=? ORDER BY id",
                (run_id,),
            ).fetchall()
        return [
            Event(
                run_id=row["run_id"],
                kind=row["kind"],
                payload=json.loads(row["payload"]),
                created_at=row["created_at"],
            )
            for row in rows
        ]

    def append_event(self, event: Event) -> None:
        with self._lock, self._connect() as connection:
            connection.execute(
                "INSERT INTO events (run_id, kind, payload, created_at) VALUES (?, ?, ?, ?)",
                (event.run_id, event.kind, json.dumps(event.payload), event.created_at.isoformat()),
            )

    def recover_interrupted_runs(self) -> int:
        with self._lock, self._connect() as connection:
            cursor = connection.execute(
                "UPDATE runs SET status=?, error=?, updated_at=? WHERE status=?",
                (
                    RunStatus.FAILED.value,
                    "Service restarted while this run was active",
                    utc_now().isoformat(),
                    RunStatus.RUNNING.value,
                ),
            )
            return cursor.rowcount

    @staticmethod
    def _state_from_row(row: sqlite3.Row) -> RunState:
        return RunState(
            run_id=row["run_id"],
            prompt=row["prompt"],
            workspace_id=row["workspace_id"],
            command=row["command"],
            status=RunStatus(row["status"]),
            result=row["result"],
            error=row["error"],
            created_at=row["created_at"],
            updated_at=row["updated_at"],
        )
