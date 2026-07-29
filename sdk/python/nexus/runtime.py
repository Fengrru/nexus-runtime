"""
Runtime — Entry point for Nexus Runtime operations.
"""
import json
import sqlite3
import uuid
import time
import os
import hashlib
import subprocess
import sys
from typing import Optional, List, Dict, Any
from dataclasses import dataclass, field
from enum import Enum

from .session import Session, SessionStatus

class DeploymentMode(Enum):
    LITE = "lite"
    PRO = "pro"
    ENTERPRISE = "enterprise"

@dataclass
class RuntimeConfig:
    mode: DeploymentMode = DeploymentMode.LITE
    db_path: str = "~/.nexus/events.db"
    vault_path: str = "~/.nexus/vault"
    max_workers: int = 4
    default_model: str = "claude-3.5-sonnet"
    signing_key: Optional[bytes] = None
    worker_path: Optional[str] = None  # path to python-worker/main.py

class Runtime:
    """Nexus Runtime — manages sessions, event store, and workers."""

    def __init__(self, **kwargs):
        self.config = RuntimeConfig(**kwargs)
        self.config.db_path = os.path.expanduser(self.config.db_path)
        self.config.vault_path = os.path.expanduser(self.config.vault_path)

        os.makedirs(os.path.dirname(self.config.db_path), exist_ok=True)
        os.makedirs(self.config.vault_path, exist_ok=True)

        self._conn = sqlite3.connect(self.config.db_path)
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._init_schema()

    def _init_schema(self):
        self._conn.executescript("""
            CREATE TABLE IF NOT EXISTS events (
                event_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                session_id TEXT NOT NULL,
                trace_id TEXT NOT NULL,
                parent_event_id TEXT,
                causal_vector TEXT NOT NULL,
                payload BLOB,
                payload_hash TEXT NOT NULL,
                event_timestamp INTEGER NOT NULL,
                nonce TEXT NOT NULL,
                integrity_hash TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_events_session
                ON events(session_id, event_timestamp);
            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                version INTEGER NOT NULL DEFAULT 1,
                status TEXT NOT NULL DEFAULT 'created',
                intent_graph BLOB,
                execution_frontier BLOB,
                memory_refs BLOB,
                budget BLOB,
                checkpoint_seq INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                latest_event_id TEXT
            );
        """)

    def create_session(
        self,
        intent: str,
        model: Optional[str] = None,
        budget_usd: float = 5.0,
        source: str = "python-sdk",
    ) -> Session:
        session_id = uuid.uuid4().hex
        model = model or self.config.default_model

        cv = {session_id: 1}
        event = {
            "event_id": f"e_{int(time.time()*1000)}_{uuid.uuid4().hex[:8]}",
            "event_type": "intent_received",
            "session_id": session_id,
            "trace_id": uuid.uuid4().hex,
            "causal_vector": json.dumps(cv),
            "payload": json.dumps({"raw_input": intent, "source": source}).encode(),
            "payload_hash": hashlib.sha256(intent.encode()).hexdigest(),
            "event_timestamp": int(time.time() * 1000),
            "nonce": uuid.uuid4().hex,
            "integrity_hash": hashlib.sha256(f"{session_id}:{intent}".encode()).hexdigest(),
        }

        self._conn.execute(
            """INSERT INTO events (event_id, event_type, session_id, trace_id,
               causal_vector, payload, payload_hash, event_timestamp,
               nonce, integrity_hash)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (event["event_id"], event["event_type"], event["session_id"],
             event["trace_id"], event["causal_vector"], event["payload"],
             event["payload_hash"], event["event_timestamp"],
             event["nonce"], event["integrity_hash"]),
        )
        self._conn.commit()

        # Also insert into sessions table so list_sessions() works immediately
        self._conn.execute(
            """INSERT OR REPLACE INTO sessions (session_id, version, status, checkpoint_seq,
               created_at, updated_at, latest_event_id, intent_graph, execution_frontier,
               memory_refs, budget)
               VALUES (?, 1, ?, 0, ?, ?, ?, ?, ?, ?, ?)""",
            (
                session_id, "created",
                event["event_timestamp"], event["event_timestamp"], event["event_id"],
                b"", b"", b"", b"",
            ),
        )
        self._conn.commit()

        return Session(
            runtime=self,
            session_id=session_id,
            intent=intent,
            model=model,
            budget_limit_cents=int(budget_usd * 100),
            status=SessionStatus.CREATED,
        )

    def resume_session(self, session_id: str) -> Optional[Session]:
        row = self._conn.execute(
            "SELECT * FROM sessions WHERE session_id = ?", (session_id,)
        ).fetchone()

        if row is None:
            events = self._conn.execute(
                "SELECT * FROM events WHERE session_id = ? ORDER BY event_timestamp",
                (session_id,),
            ).fetchall()
            if not events:
                return None
            return Session(
                runtime=self,
                session_id=session_id,
                intent="recovered",
                model=self.config.default_model,
                budget_limit_cents=500,
                status=SessionStatus.CREATED,
                checkpoint_seq=0,
            )

        return Session(
            runtime=self,
            session_id=session_id,
            intent="resumed",
            model=self.config.default_model,
            budget_limit_cents=500,
            status=SessionStatus(row[2]),
            checkpoint_seq=row[6] or 0,
        )

    def list_sessions(self, status: Optional[str] = None) -> List[Dict[str, Any]]:
        if status:
            rows = self._conn.execute(
                "SELECT session_id, status, version, checkpoint_seq, created_at FROM sessions WHERE status = ?",
                (status,),
            ).fetchall()
        else:
            rows = self._conn.execute(
                "SELECT session_id, status, version, checkpoint_seq, created_at FROM sessions"
            ).fetchall()

        return [
            {
                "session_id": r[0],
                "status": r[1],
                "version": r[2],
                "checkpoint_seq": r[3],
                "created_at": r[4],
            }
            for r in rows
        ]

    def get_events(self, session_id: str, limit: int = 20) -> List[Dict]:
        rows = self._conn.execute(
            "SELECT event_id, event_type, causal_vector, event_timestamp FROM events WHERE session_id = ? ORDER BY event_timestamp LIMIT ?",
            (session_id, limit),
        ).fetchall()
        return [
            {"event_id": r[0], "event_type": r[1], "causal_vector": r[2], "timestamp": r[3]}
            for r in rows
        ]

    def export_session(self, session_id: str, output_path: str) -> str:
        events = self._conn.execute(
            "SELECT event_id, event_type, causal_vector, payload_hash, event_timestamp FROM events WHERE session_id = ? ORDER BY event_timestamp",
            (session_id,),
        ).fetchall()

        export = {
            "version": "1.0.0",
            "session_id": session_id,
            "events": [
                {
                    "event_id": e[0],
                    "event_type": e[1],
                    "causal_vector": e[2],
                    "payload_hash": e[3],
                    "timestamp": e[4],
                }
                for e in events
            ],
        }

        with open(output_path, "w") as f:
            json.dump(export, f, indent=2)

        return output_path

    def import_session(self, file_path: str) -> Optional[Session]:
        with open(file_path, "r") as f:
            export = json.load(f)

        session_id = export["session_id"]
        for event in export.get("events", []):
            self._conn.execute(
                """INSERT OR IGNORE INTO events (event_id, event_type, session_id,
                   causal_vector, payload_hash, event_timestamp, nonce, integrity_hash,
                   trace_id)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    event["event_id"], event["event_type"], session_id,
                    event.get("causal_vector", "{}"), event.get("payload_hash", ""),
                    event.get("timestamp", 0), uuid.uuid4().hex, uuid.uuid4().hex,
                    uuid.uuid4().hex,
                ),
            )
        self._conn.commit()

        return self.resume_session(session_id)

    def _resolve_worker_path(self) -> str:
        """Resolve the path to the Python worker executable."""
        if self.config.worker_path:
            return os.path.expanduser(self.config.worker_path)

        # Try environment variable
        env_path = os.environ.get("NEXUS_WORKER_PATH")
        if env_path and os.path.isfile(env_path):
            return env_path

        # Search candidates relative to this package file
        pkg_dir = os.path.dirname(os.path.abspath(__file__))
        candidates = [
            os.path.normpath(os.path.join(pkg_dir, "..", "..", "..", "workers", "python-worker", "main.py")),
            os.path.normpath(os.path.join(pkg_dir, "..", "..", "workers", "python-worker", "main.py")),
        ]
        # Also search from NEXUS_ROOT env var
        if "NEXUS_ROOT" in os.environ:
            candidates.append(
                os.path.normpath(os.path.join(os.environ["NEXUS_ROOT"], "workers", "python-worker", "main.py"))
            )

        for candidate in candidates:
            if os.path.isfile(candidate):
                return candidate

        raise FileNotFoundError(
            "Cannot find python-worker/main.py. "
            "Set RuntimeConfig.worker_path or NEXUS_WORKER_PATH to the correct path. "
            f"Searched: {candidates}"
        )

    def _spawn_worker(
        self, session: "Session", task_id: str, intent_payload: Dict[str, Any]
    ) -> Dict[str, Any]:
        """Spawn a worker subprocess and execute a task via JSON-RPC 2.0.

        Returns:
            Dict with keys: status, artifacts, metrics, events
        """
        worker_path = self._resolve_worker_path()

        proc = subprocess.Popen(
            [sys.executable, worker_path],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )

        request = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "execute",
            "params": {
                "task_id": task_id,
                "session_id": session.session_id,
                "capabilities": intent_payload.get("capabilities", []),
                "intent": intent_payload,
                "inputs": intent_payload.get("inputs", []),
            },
        }

        # Send execute request
        proc.stdin.write(json.dumps(request) + "\n")
        proc.stdin.flush()

        events: List[Dict] = []
        final_result: Optional[Dict[str, Any]] = None
        checkpoint_count = 0

        # Read JSON-RPC responses (NDJSON) — break on result/error
        while True:
            line = proc.stdout.readline()
            if not line:
                break
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue

            if "result" in msg:
                final_result = msg["result"]
                break
            elif "error" in msg:
                final_result = {"status": "failed", "error": msg["error"]}
                break
            elif msg.get("method") == "checkpoint":
                checkpoint_count += 1
                params = msg.get("params", {})
                events.append({
                    "type": "checkpoint",
                    "step": params.get("step_index", checkpoint_count),
                    "progress": params.get("progress_percent", 0),
                    "actions": params.get("actions", []),
                })
            elif msg.get("method") == "progress":
                params = msg.get("params", {})
                events.append({
                    "type": "progress",
                    "percent": params.get("percent", 0),
                    "step": params.get("current_step", ""),
                })

        # Close stdin to signal worker shutdown
        proc.stdin.close()

        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()

        if final_result is None:
            final_result = {
                "status": "failed",
                "error": "Worker exited without returning a result",
            }

        # Capture stderr for diagnostics
        stderr_output = proc.stderr.read()
        if stderr_output:
            events.append({"type": "worker_log", "content": stderr_output[:2000]})

        return {
            "status": final_result.get("status", "failed"),
            "artifacts": final_result.get("artifacts", []),
            "metrics": final_result.get("metrics", {}),
            "error": final_result.get("error"),
            "events": events,
            "checkpoint_count": checkpoint_count,
        }

    def close(self):
        self._conn.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
