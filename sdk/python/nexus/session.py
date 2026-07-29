"""
Session — Represents a single Nexus execution session.
"""
import json
import time
import uuid
from typing import Optional, List, Dict, Any, TYPE_CHECKING
from enum import Enum
from dataclasses import dataclass, field

if TYPE_CHECKING:
    from .runtime import Runtime

class SessionStatus(Enum):
    CREATED = "created"
    INTAKE = "intake"
    PLANNING = "planning"
    PLANNED = "planned"
    EXECUTING = "executing"
    CHECKPOINTING = "checkpointing"
    BLOCKED = "blocked"
    CONVERGING = "converging"
    REFLECTING = "reflecting"
    COMPLETED = "completed"
    FAILED = "failed"
    ARCHIVED = "archived"

@dataclass
class Session:
    runtime: "Runtime"
    session_id: str
    intent: str
    model: str
    budget_limit_cents: int
    status: SessionStatus = SessionStatus.CREATED
    checkpoint_seq: int = 0
    _worker_result: Optional[Dict[str, Any]] = field(default=None, repr=False)

    @property
    def id(self) -> str:
        return self.session_id

    @property
    def result(self) -> Optional[Dict[str, Any]]:
        """Return the worker execution result, or None if not yet run."""
        return self._worker_result

    @property
    def succeeded(self) -> bool:
        """True if the session completed successfully."""
        return self.status == SessionStatus.COMPLETED

    def run(self, plan_steps: Optional[List[Dict]] = None) -> "Session":
        """Execute the session by spawning a real worker subprocess.

        The worker is invoked via JSON-RPC 2.0 over stdio (NDJSON framing).
        If plan_steps is provided, it is sent as the execution plan;
        otherwise the raw intent string is parsed into a single-step plan.
        """
        # Intake phase
        self._transition(SessionStatus.INTAKE)

        # Build the intent payload for the worker
        if plan_steps:
            intent_payload = {
                "action_type": "execute_plan",
                "target": "",
                "parameters": {"plan": json.dumps(plan_steps)},
            }
        else:
            # Parse intent string into a simple action dispatch
            intent_payload = self._parse_intent(self.intent)

        # Planning phase
        self._transition(SessionStatus.PLANNING)

        # Phase checkpoint before execution
        self._transition(SessionStatus.PLANNED)

        # Spawn worker and execute
        task_id = uuid.uuid4().hex
        self._transition(SessionStatus.EXECUTING)

        try:
            result = self.runtime._spawn_worker(self, task_id, intent_payload)
        except FileNotFoundError as e:
            self._worker_result = {
                "status": "failed",
                "error": str(e),
                "artifacts": [],
                "metrics": {},
                "events": [],
                "checkpoint_count": 0,
            }
            self._transition(SessionStatus.FAILED)
            return self

        self._worker_result = result

        # Persist checkpoint events from the worker
        for evt in result.get("events", []):
            if evt.get("type") == "checkpoint":
                self._checkpoint(evt.get("step", 1))

        # Final status
        if result.get("status") == "completed":
            self._transition(SessionStatus.COMPLETED)
        else:
            self._transition(SessionStatus.FAILED)

        return self

    def _parse_intent(self, intent_text: str) -> Dict[str, Any]:
        """Parse a natural-language intent into an action dispatch.

        Simple keyword matching — in production this would use the LLM planner.
        """
        lower = intent_text.lower()

        # Check for multi-step markers
        if "{" in intent_text and "action_type" in intent_text:
            try:
                return json.loads(intent_text)
            except json.JSONDecodeError:
                pass

        # Read file
        if any(w in lower for w in ["read", "show", "display", "cat", "view", "inspect"]):
            # Try to extract a file path (Unix and Windows)
            import re
            path_match = re.search(r'["\']?([^\s"\'<>|?*]*\.\w{1,10})["\']?', intent_text)
            if path_match:
                return {
                    "action_type": "read_file",
                    "target": path_match.group(1),
                    "parameters": {},
                }

        # Write file
        if any(w in lower for w in ["write", "create", "generate", "save"]):
            import re
            path_match = re.search(r'["\']?([^\s"\'<>|?*]*\.\w{1,10})["\']?', intent_text)
            return {
                "action_type": "write_file",
                "target": path_match.group(1) if path_match else "output.txt",
                "parameters": {"content": intent_text},
            }

        # Search/grep
        if any(w in lower for w in ["search", "find", "grep", "locate", "look for"]):
            import re
            path_match = re.search(r'["\']?([^\s"\'<>|?*]*\.\w{1,10})["\']?', intent_text)
            return {
                "action_type": "grep",
                "target": path_match.group(1) if path_match else ".",
                "parameters": {"pattern": intent_text.split()[-1] if intent_text.split() else ""},
            }

        # Run command
        if any(w in lower for w in ["run", "execute", "command", "test", "build", "compile"]):
            return {
                "action_type": "run_command",
                "target": intent_text,
                "parameters": {"command": intent_text},
            }

        # Default: treat as a read action on the intent itself
        return {
            "action_type": "read_file",
            "target": intent_text,
            "parameters": {},
        }

    def _transition(self, status: SessionStatus):
        from .event import NexusEvent
        event = NexusEvent(
            event_id=f"e_{int(time.time()*1000)}_{id(self)}",
            event_type=f"session_{status.value}",
            session_id=self.session_id,
            causal_vector=json.dumps({self.session_id: self.checkpoint_seq + 1}),
        )
        self._persist_event(event)
        self.status = status

    def _checkpoint(self, step: int):
        from .event import NexusEvent
        self.checkpoint_seq = step
        event = NexusEvent(
            event_id=f"cp_{int(time.time()*1000)}_{step}",
            event_type="worker_checkpoint",
            session_id=self.session_id,
            causal_vector=json.dumps({self.session_id: step}),
        )
        self._persist_event(event)

    def _persist_event(self, event: "NexusEvent"):
        self.runtime._conn.execute(
            """INSERT OR IGNORE INTO events (event_id, event_type, session_id, trace_id,
               causal_vector, payload, payload_hash, event_timestamp, nonce, integrity_hash)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                event.event_id, event.event_type, event.session_id,
                event.trace_id or "0"*32, event.causal_vector,
                event.payload or b"", event.payload_hash or "",
                event.event_timestamp or int(time.time()*1000),
                event.nonce or "0"*32, event.integrity_hash or "",
            ),
        )
        self.runtime._conn.execute(
            """INSERT OR REPLACE INTO sessions (session_id, version, status, checkpoint_seq,
               created_at, updated_at, latest_event_id, intent_graph, execution_frontier,
               memory_refs, budget)
               VALUES (?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                self.session_id, self.status.value, self.checkpoint_seq,
                int(time.time()*1000), int(time.time()*1000), event.event_id,
                b"", b"", b"", b"",
            ),
        )
        self.runtime._conn.commit()

    def suspend(self):
        self._transition(SessionStatus.CHECKPOINTING)

    def resume(self):
        self._transition(SessionStatus.EXECUTING)

    def block(self, reason: str):
        self._transition(SessionStatus.BLOCKED)

    def approve(self):
        self._transition(SessionStatus.EXECUTING)

    def reject(self):
        self._transition(SessionStatus.FAILED)

    def archive(self):
        self._transition(SessionStatus.ARCHIVED)

    def get_events(self, limit: int = 50) -> List[Dict]:
        return self.runtime.get_events(self.session_id, limit)

    def to_dict(self) -> Dict[str, Any]:
        return {
            "session_id": self.session_id,
            "intent": self.intent,
            "model": self.model,
            "status": self.status.value,
            "checkpoint_seq": self.checkpoint_seq,
            "budget_limit_cents": self.budget_limit_cents,
        }
