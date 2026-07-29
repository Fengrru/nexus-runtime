"""
Nexus Runtime — Python SDK v1.0.0

Causally-consistent, crash-recoverable, deterministic execution substrate.

Usage:
    from nexus import Runtime

    runtime = Runtime(mode="lite", db_path="~/.nexus/events.db")
    session = runtime.create_session(intent="refactor auth", budget_usd=5.00)
    session.wait()
"""
from .runtime import Runtime
from .session import Session, SessionStatus
from .memory import Memory, MemoryGraph, MemoryContent, MemoryContentType, MemoryEdgeType
from .budget import Budget
from .event import EventType, NexusEvent

__version__ = "1.0.0"
__all__ = [
    "Runtime", "Session", "SessionStatus",
    "Memory", "MemoryGraph", "MemoryContent", "MemoryContentType", "MemoryEdgeType",
    "Budget", "EventType", "NexusEvent",
]
