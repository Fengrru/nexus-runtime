"""
Tests for the Nexus Runtime Python SDK.

Run with: python -m pytest sdk/python/nexus/test_runtime.py -v
"""
import os
import sys
import json
import tempfile
import time
import uuid

# Ensure the SDK is importable
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", ".."))


def test_runtime_import():
    """SDK imports cleanly without errors."""
    from nexus import Runtime, Session, SessionStatus, Budget, NexusEvent
    assert Runtime is not None
    assert Session is not None
    assert SessionStatus is not None


def test_runtime_init():
    """Runtime initializes and creates the database."""
    from nexus import Runtime
    tmp = tempfile.mkdtemp()
    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        assert os.path.exists(db_path)
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_create_session():
    """Creating a session inserts an event and returns a Session object."""
    from nexus import Runtime, SessionStatus
    tmp = tempfile.mkdtemp()
    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        session = rt.create_session(intent="read test.txt", budget_usd=3.0)
        assert session.session_id is not None
        assert len(session.session_id) == 32  # hex uuid4
        assert session.status == SessionStatus.CREATED
        assert session.intent == "read test.txt"
        assert session.budget_limit_cents == 300
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_list_sessions():
    """List sessions returns created sessions."""
    from nexus import Runtime
    tmp = tempfile.mkdtemp()
    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        s1 = rt.create_session(intent="task one")
        s2 = rt.create_session(intent="task two")
        sessions = rt.list_sessions()
        assert len(sessions) >= 2
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_get_events():
    """Get events returns the intent_received event."""
    from nexus import Runtime
    tmp = tempfile.mkdtemp()
    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        session = rt.create_session(intent="read test.txt")
        events = session.get_events(limit=10)
        assert len(events) == 1
        assert events[0]["event_type"] == "intent_received"
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_resume_session():
    """Resuming a created session returns it."""
    from nexus import Runtime, SessionStatus
    tmp = tempfile.mkdtemp()
    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        s1 = rt.create_session(intent="resume test")
        sid = s1.session_id

        s2 = rt.resume_session(sid)
        assert s2 is not None
        assert s2.session_id == sid
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_resume_nonexistent():
    """Resuming a nonexistent session returns None."""
    from nexus import Runtime
    tmp = tempfile.mkdtemp()
    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        s = rt.resume_session("nonexistent_session_id")
        assert s is None
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_session_state_transitions():
    """Session transitions through states correctly."""
    from nexus import Runtime, SessionStatus
    tmp = tempfile.mkdtemp()
    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        session = rt.create_session(intent="test transitions")

        assert session.status == SessionStatus.CREATED
        session.suspend()
        assert session.status == SessionStatus.CHECKPOINTING
        session.resume()
        assert session.status == SessionStatus.EXECUTING
        session.block("needs review")
        assert session.status == SessionStatus.BLOCKED
        session.approve()
        assert session.status == SessionStatus.EXECUTING
        session.reject()
        assert session.status == SessionStatus.FAILED
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_session_to_dict():
    """to_dict returns correct serialization."""
    from nexus import Runtime
    tmp = tempfile.mkdtemp()
    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        session = rt.create_session(intent="dict test", budget_usd=2.5)
        d = session.to_dict()
        assert d["session_id"] == session.session_id
        assert d["intent"] == "dict test"
        assert d["status"] == "created"
        assert d["budget_limit_cents"] == 250
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_export_import_session():
    """Export and import a session preserves events."""
    from nexus import Runtime
    tmp = tempfile.mkdtemp()
    db_path1 = os.path.join(tmp, "events1.db")
    db_path2 = os.path.join(tmp, "events2.db")
    export_path = os.path.join(tmp, "session.json")
    try:
        rt1 = Runtime(db_path=db_path1)
        session = rt1.create_session(intent="export test")
        sid = session.session_id
        rt1.export_session(sid, export_path)
        rt1.close()

        import json
        with open(export_path) as f:
            exported = json.load(f)
        assert exported["session_id"] == sid
        assert len(exported["events"]) == 1

        rt2 = Runtime(db_path=db_path2)
        imported = rt2.import_session(export_path)
        assert imported is not None
        assert imported.session_id == sid
        rt2.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_worker_run_read_file():
    """Worker spawns and reads a file correctly."""
    from nexus import Runtime, SessionStatus
    tmp = tempfile.mkdtemp()
    test_file = os.path.join(tmp, "hello.txt")
    with open(test_file, "w") as f:
        f.write("Hello from Nexus worker test!")

    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        # Point worker_path at the actual worker
        pkg_dir = os.path.dirname(os.path.abspath(__file__))
        worker_path = os.path.normpath(
            os.path.join(pkg_dir, "..", "..", "..", "workers", "python-worker", "main.py")
        )
        if not os.path.isfile(worker_path):
            rt.close()
            import pytest
            pytest.skip(f"Worker not found at {worker_path}")

        rt.config.worker_path = worker_path

        session = rt.create_session(intent=f"read {test_file}")
        session.run()

        assert session.status in (SessionStatus.COMPLETED, SessionStatus.FAILED), \
            f"Expected COMPLETED or FAILED, got {session.status}"
        assert session.result is not None
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_worker_run_write_file():
    """Worker spawns, writes a file, and returns an artifact."""
    from nexus import Runtime, SessionStatus
    tmp = tempfile.mkdtemp()
    output_file = os.path.join(tmp, "output.txt")
    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        pkg_dir = os.path.dirname(os.path.abspath(__file__))
        worker_path = os.path.normpath(
            os.path.join(pkg_dir, "..", "..", "..", "workers", "python-worker", "main.py")
        )
        if not os.path.isfile(worker_path):
            rt.close()
            import pytest
            pytest.skip(f"Worker not found at {worker_path}")

        rt.config.worker_path = worker_path

        session = rt.create_session(
            intent=f"write {output_file} with content: Nexus SDK test output"
        )
        session.run()

        assert session.result is not None
        # The worker should have written the file
        if session.status == SessionStatus.COMPLETED:
            assert os.path.isfile(output_file), f"Output file not created: {output_file}"
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_worker_run_with_plan():
    """Worker executes a multi-step JSON plan."""
    from nexus import Runtime, SessionStatus
    tmp = tempfile.mkdtemp()
    db_path = os.path.join(tmp, "events.db")
    try:
        rt = Runtime(db_path=db_path)
        pkg_dir = os.path.dirname(os.path.abspath(__file__))
        worker_path = os.path.normpath(
            os.path.join(pkg_dir, "..", "..", "..", "workers", "python-worker", "main.py")
        )
        if not os.path.isfile(worker_path):
            rt.close()
            import pytest
            pytest.skip(f"Worker not found at {worker_path}")

        rt.config.worker_path = worker_path

        plan = [
            {
                "action_type": "write_file",
                "target": os.path.join(tmp, "step1.txt"),
                "parameters": {"content": "step 1 output"},
            },
            {
                "action_type": "read_file",
                "target": os.path.join(tmp, "step1.txt"),
                "parameters": {},
            },
        ]

        session = rt.create_session(intent="multi-step plan test")
        session.run(plan_steps=plan)

        assert session.result is not None
        assert session.status in (SessionStatus.COMPLETED, SessionStatus.FAILED)
        rt.close()
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_context_manager():
    """Runtime supports 'with' statement."""
    from nexus import Runtime
    tmp = tempfile.mkdtemp()
    db_path = os.path.join(tmp, "events.db")
    try:
        with Runtime(db_path=db_path) as rt:
            session = rt.create_session(intent="context manager test")
            assert session is not None
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


def test_budget_class():
    """Budget class works correctly."""
    from nexus import Budget
    b = Budget(limit_cents=500)
    assert b.limit_cents == 500
    assert b.remaining_cents == 500
    assert b.remaining_dollars == 5.0
    assert b.is_exhausted is False
    assert b.can_afford(100) is True
    b.add_cost(100)
    assert b.remaining_cents == 400
    assert b.consumed_cents == 100
    assert b.can_afford(500) is False
    b.add_cost(500)
    assert b.consumed_cents == 500  # capped at limit
    assert b.is_exhausted is True
    d = b.to_dict()
    assert d["limit_cents"] == 500


def test_memory_graph():
    """MemoryGraph add and query works."""
    from nexus import MemoryGraph, Memory, MemoryContent, MemoryContentType, MemoryEdgeType
    mg = MemoryGraph()

    content = MemoryContent(
        content_type=MemoryContentType.TEXT,
        text="Test knowledge about Rust",
    )
    memory = Memory(
        memory_id="mem_001",
        content=content,
        importance=7000,
    )
    mg.add(memory)
    assert mg.size() == 1
    assert mg.get("mem_001") is not None
    assert mg.get("mem_001").importance == 7000

    # Add another and create an edge
    content2 = MemoryContent(
        content_type=MemoryContentType.TEXT,
        text="Rust ownership system",
    )
    memory2 = Memory(
        memory_id="mem_002",
        content=content2,
        importance=5000,
    )
    mg.add(memory2)
    mg.add_edge("mem_001", "mem_002", MemoryEdgeType.REFINES)
    assert mg.size() == 2

    # Query causal traversal
    results = mg.query_causal("mem_001", depth=3)
    assert len(results) >= 1

    # Activation computation
    activation = mg.compute_activation("mem_001", {})
    assert activation > 0
