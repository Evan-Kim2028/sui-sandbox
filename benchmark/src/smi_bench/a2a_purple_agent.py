from __future__ import annotations

import argparse
import json
import signal
import sys
from typing import Any, cast

import uvicorn
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.apps import A2AStarletteApplication
from a2a.server.events import EventQueue
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore, TaskUpdater
from a2a.types import AgentCapabilities, AgentCard, AgentProvider, AgentSkill, Part, TaskState, TextPart
from a2a.utils import new_agent_text_message, new_task
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import Response

# A2A Protocol version this implementation supports
A2A_PROTOCOL_VERSION = "0.3.0"


def _card(*, url: str) -> AgentCard:
    skill = AgentSkill(
        id="baseline",
        name="Baseline",
        description="Baseline purple agent (stub) for AgentBeats wiring tests.",
        tags=["baseline"],
        examples=["ping"],
        input_modes=["text/plain", "application/json"],
        output_modes=["text/plain", "application/json"],
    )
    return AgentCard(
        name="smi-bench-purple",
        description="Baseline purple agent for AgentBeats (stub).",
        url=url,
        version="0.1.0",
        protocol_version=A2A_PROTOCOL_VERSION,  # Add explicit A2A protocol version
        provider=AgentProvider(organization="sui-move-interface-extractor", url=url),
        default_input_modes=["text/plain", "application/json"],
        default_output_modes=["text/plain", "application/json"],
        capabilities=AgentCapabilities(streaming=True, push_notifications=False, state_transition_history=False),
        skills=[skill],
    )


class PurpleExecutor(AgentExecutor):
    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        task = context.current_task
        if task is None:
            if context.message is None:
                raise ValueError("RequestContext.message is missing")
            task = new_task(context.message)
            await event_queue.enqueue_event(task)
        updater = TaskUpdater(event_queue, task.id, task.context_id)

        await updater.update_status(TaskState.working, new_agent_text_message("ready", task.context_id, task.id))

        raw = context.get_user_input()
        payload: Any
        try:
            payload = json.loads(raw) if raw else raw
        except json.JSONDecodeError:
            payload = raw

        reply = {"ok": True, "echo": payload}
        await updater.add_artifact([Part(root=TextPart(text=json.dumps(reply, sort_keys=True)))], name="response")
        await updater.complete()

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        """
        Purple agent (stub) doesn't support cancellation.
        Tasks complete immediately, so cancellation is not applicable.
        """
        task = context.current_task
        if task is None:
            raise ValueError("No current task to cancel")

        # Purple agent tasks complete immediately, so they're always in terminal state
        raise RuntimeError(f"Task {task.id} cannot be cancelled (purple agent tasks are immediate)")


class A2AVersionMiddleware(BaseHTTPMiddleware):
    """
    Middleware to add A2A-Version header to all responses.
    Implements A2A protocol version signaling per spec section 14.2.1.
    """

    async def dispatch(self, request: Request, call_next: Any) -> Response:
        response = await call_next(request)
        response.headers["A2A-Version"] = A2A_PROTOCOL_VERSION
        return response


def build_app(*, public_url: str) -> Any:
    handler = DefaultRequestHandler(agent_executor=PurpleExecutor(), task_store=InMemoryTaskStore())
    app = A2AStarletteApplication(agent_card=_card(url=public_url), http_handler=handler).build()

    # Add A2A version header middleware
    app.add_middleware(cast(Any, A2AVersionMiddleware))

    return app


def _setup_signal_handlers() -> None:
    """
    Set up signal handlers for graceful shutdown.
    """

    def handler(signum: int, frame: Any) -> None:
        sys.exit(128 + signum)

    signal.signal(signal.SIGTERM, handler)
    signal.signal(signal.SIGINT, handler)


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(description="A2A baseline purple agent (stub)")
    p.add_argument("--host", type=str, default="0.0.0.0")
    p.add_argument("--port", type=int, default=9998)
    p.add_argument("--card-url", type=str, default=None)
    args = p.parse_args(argv)

    # Set up signal handlers before starting server
    _setup_signal_handlers()

    url = args.card_url or f"http://{args.host}:{args.port}/"
    app = build_app(public_url=url)
    uvicorn.run(app, host=args.host, port=args.port)


if __name__ == "__main__":
    main()
