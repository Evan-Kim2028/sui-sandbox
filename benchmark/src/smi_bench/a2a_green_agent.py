from __future__ import annotations

import argparse
import asyncio
import collections
import inspect
import json
import os
import time
from dataclasses import dataclass
from pathlib import Path
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
from starlette.responses import JSONResponse, Response

from smi_bench.a2a_errors import A2AError, InvalidConfigError, TaskNotCancelableError
from smi_bench.schema import Phase2ResultKeys
from smi_bench.utils import safe_json_loads

# A2A Protocol version this implementation supports
A2A_PROTOCOL_VERSION = "0.3.0"

# Supported content types (for future content validation)
SUPPORTED_CONTENT_TYPES = {"application/json", "text/plain"}


@dataclass(frozen=True)
class EvalConfig:
    corpus_root: str
    package_ids_file: str
    samples: int
    agent: str
    rpc_url: str
    simulation_mode: str
    per_package_timeout_seconds: float
    max_plan_attempts: int
    continue_on_error: bool
    resume: bool
    run_id: str | None


def _safe_int(v: Any, default: int) -> int:
    try:
        return int(v)
    except Exception:
        return default


def _safe_float(v: Any, default: float) -> float:
    try:
        return float(v)
    except Exception:
        return default


def _load_cfg(raw: Any) -> EvalConfig:
    if not isinstance(raw, dict):
        raise InvalidConfigError("config", "must be a JSON object")

    corpus_root = str(raw.get("corpus_root") or "")
    package_ids_file = str(raw.get("package_ids_file") or raw.get("manifest") or "")

    if not corpus_root:
        raise InvalidConfigError("corpus_root", "missing or empty")
    if not package_ids_file:
        raise InvalidConfigError("package_ids_file", "missing or empty")

    return EvalConfig(
        corpus_root=corpus_root,
        package_ids_file=package_ids_file,
        samples=_safe_int(raw.get("samples"), 0),
        agent=str(raw.get("agent") or "real-openai-compatible"),
        rpc_url=str(raw.get("rpc_url") or "https://fullnode.mainnet.sui.io:443"),
        simulation_mode=str(raw.get("simulation_mode") or "dry-run"),
        per_package_timeout_seconds=_safe_float(raw.get("per_package_timeout_seconds"), 300.0),
        max_plan_attempts=_safe_int(raw.get("max_plan_attempts"), 2),
        continue_on_error=bool(raw.get("continue_on_error", True)),
        resume=bool(raw.get("resume", True)),
        run_id=str(raw.get("run_id")) if raw.get("run_id") else None,
    )


def _extract_payload(context: RequestContext) -> dict[str, Any]:
    raw = context.get_user_input()
    if raw:
        try:
            v = safe_json_loads(raw, context="user input payload")
            if isinstance(v, dict):
                return v
        except Exception:
            pass

    params = getattr(context, "_params", None)
    if params is not None:
        meta = getattr(params, "metadata", None)
        if isinstance(meta, dict):
            return meta

    return {}


def _summarize_phase2_results(out_json: Path) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    try:
        data = safe_json_loads(out_json.read_text(encoding="utf-8"), context="phase2 results")
    except Exception:
        return {}, []

    # Return as much as we can even if some fields are missing.
    if not isinstance(data, dict):
        return {}, []

    aggregate = data.get("aggregate")
    packages = data.get("packages")

    k = Phase2ResultKeys
    metrics: dict[str, Any] = {}
    if isinstance(aggregate, dict):
        metrics[k.AVG_HIT_RATE] = aggregate.get(k.AVG_HIT_RATE)
        metrics[k.ERRORS] = aggregate.get(k.ERRORS)
        # New aggregate metrics (present in schema_version>=2 when enabled)
        for key in (
            k.PLANNING_ONLY_HIT_RATE,
            k.PLANNING_ONLY_PACKAGES,
            k.FORMATTING_ONLY_FAILURES,
            k.CAUSALITY_SUCCESS_RATE,
            k.FORMATTING_CORRECTIONS_HISTOGRAM,
            k.TOTAL_PROMPT_TOKENS,
            k.TOTAL_COMPLETION_TOKENS,
        ):
            if key in aggregate:
                metrics[key] = aggregate.get(key)

    error_rows: list[dict[str, Any]] = []
    if isinstance(packages, list):
        for row in packages:
            if not isinstance(row, dict):
                continue
            err = row.get(k.ERROR)
            timed_out = row.get(k.TIMED_OUT)
            if err or timed_out:
                score = row.get(k.SCORE) if isinstance(row.get(k.SCORE), dict) else {}
                error_rows.append(
                    {
                        k.PACKAGE_ID: row.get(k.PACKAGE_ID),
                        k.ERROR: err,
                        k.TIMED_OUT: timed_out,
                        k.ELAPSED_SECONDS: row.get(k.ELAPSED_SECONDS),
                        k.PLAN_ATTEMPTS: row.get(k.PLAN_ATTEMPTS),
                        k.SIM_ATTEMPTS: row.get(k.SIM_ATTEMPTS),
                        k.SCORE: {
                            "targets": score.get("targets"),
                            "created_hits": score.get("created_hits"),
                            "created_distinct": score.get("created_distinct"),
                        },
                        # New per-package intelligence signals
                        k.PTB_PARSE_OK: row.get(k.PTB_PARSE_OK),
                        k.FORMATTING_CORRECTIONS: row.get(k.FORMATTING_CORRECTIONS),
                        k.CAUSALITY_VALID: row.get(k.CAUSALITY_VALID),
                        k.CAUSALITY_SCORE: row.get(k.CAUSALITY_SCORE),
                        k.CAUSALITY_ERRORS: row.get(k.CAUSALITY_ERRORS),
                    }
                )

    if isinstance(packages, list):
        metrics["packages_total"] = len(packages)
        metrics["packages_with_error"] = len(error_rows)
        metrics["packages_timed_out"] = sum(1 for e in error_rows if e.get(k.TIMED_OUT))

    return metrics, error_rows


def _summarize_failure_modes(errors: list[dict[str, Any]]) -> dict[str, int]:
    """
    Analyze error rows and categorize common failure modes for rapid diagnosis.
    """
    summary: dict[str, int] = {}

    for row in errors:
        err = str(row.get("error") or "").lower()
        if not err:
            if row.get("timed_out"):
                summary["timeout"] = summary.get("timeout", 0) + 1
            continue

        # Categorization logic based on known error patterns
        if "rpc" in err or "http" in err or "connection" in err:
            key = "infrastructure_rpc_failure"
        elif "rust extractor failed" in err:
            key = "infrastructure_extractor_failure"
        elif "missing field calls" in err or "json object" in err:
            key = "model_schema_violation"
        elif "causality" in err or "result reference" in err:
            key = "model_logic_causality_error"
        elif "bad_magic" in err or "binary header" in err:
            key = "data_corruption_bad_magic"
        else:
            # Group unique/rare errors by their first few words to avoid fragmentation
            first_words = " ".join(err.split()[:3]).replace(":", "")
            key = f"other_{first_words}"

        summary[key] = summary.get(key, 0) + 1

    return summary


def _read_json(path: Path) -> dict[str, Any] | None:
    try:
        data = safe_json_loads(path.read_text(encoding="utf-8"), context=f"reading {path}")
    except Exception:
        return None
    if not isinstance(data, dict):
        return None
    return data


def _card(*, url: str) -> AgentCard:
    skill = AgentSkill(
        id="run_phase2",
        name="Run Phase II",
        description="Run Phase II (PTB inhabitation) over a manifest and return results as artifacts.",
        tags=["benchmark", "sui", "move", "phase2"],
        examples=["Run Phase II on standard manifest"],
        input_modes=["application/json"],
        output_modes=["application/json"],
    )

    return AgentCard(
        name="smi-bench-green",
        description="Green agent wrapper for the Sui Move Interface Extractor benchmark (Phase II).",
        url=url,
        version="0.1.0",
        protocol_version=A2A_PROTOCOL_VERSION,  # Add explicit A2A protocol version
        provider=AgentProvider(organization="sui-move-interface-extractor", url=url),
        default_input_modes=["application/json"],
        default_output_modes=["application/json"],
        capabilities=AgentCapabilities(streaming=True, push_notifications=False, state_transition_history=False),
        skills=[skill],
    )


class SmiBenchGreenExecutor(AgentExecutor):
    def __init__(self) -> None:
        super().__init__()
        # Track running subprocesses for cancellation support
        self._task_processes: dict[str, asyncio.subprocess.Process] = {}
        self._task_cancel_events: dict[str, asyncio.Event] = {}
        # Limit concurrency to prevent OOM and RPC stampedes
        self._concurrency_semaphore: asyncio.Semaphore | None = None

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        if self._concurrency_semaphore is None:
            self._concurrency_semaphore = asyncio.Semaphore(1)

        task = context.current_task
        if task is None:
            if context.message is None:
                raise ValueError("RequestContext.message is missing")
            task = new_task(context.message)
            context.current_task = task  # CRITICAL: assign to context
            await event_queue.enqueue_event(task)
        updater = TaskUpdater(event_queue, task.id, task.context_id)

        # Use a semaphore to ensure only one task runs at a time
        async with self._concurrency_semaphore:
            max_infra_retries = 3
            for attempt in range(max_infra_retries):
                # Create cancellation event for this task
                cancel_event = asyncio.Event()
                self._task_cancel_events[task.id] = cancel_event

                started_at = time.time()
                await updater.update_status(
                    TaskState.working,
                    new_agent_text_message(
                        f"starting (attempt {attempt + 1}/{max_infra_retries})" if attempt > 0 else "starting",
                        task.context_id,
                        task.id,
                    ),
                )

                try:
                    await self._run_task_logic(context, updater, cancel_event, started_at)
                    # If we reach here without exception, the task finished (successfully or with model errors)
                    return
                except A2AError as e:
                    # Specific protocol errors should not be retried, but must be reported
                    self._task_processes.pop(task.id, None)
                    self._task_cancel_events.pop(task.id, None)
                    await updater.failed(new_agent_text_message(str(e), task.context_id, task.id))
                    return
                except Exception as e:
                    err_str = str(e).lower()
                    is_infra_failure = any(
                        x in err_str
                        for x in [
                            "rpc",
                            "timeout",
                            "connection",
                            "http",
                            "network",
                            "no such file",
                            "not found",
                            "enoent",
                        ]
                    )

                    # If it's a transient infra failure and we have retries left, loop again
                    if is_infra_failure and attempt < max_infra_retries - 1:
                        await updater.update_status(
                            TaskState.working,
                            new_agent_text_message(
                                f"Infrastructure failure detected: {e}. Retrying in 5s...",
                                task.context_id,
                                task.id,
                            ),
                        )
                        await asyncio.sleep(5)
                        continue

                    # Otherwise, fail permanently
                    self._task_processes.pop(task.id, None)
                    self._task_cancel_events.pop(task.id, None)
                    await updater.failed(
                        new_agent_text_message(
                            f"unexpected error: {type(e).__name__}: {e}",
                            task.context_id,
                            task.id,
                        )
                    )
                    return

    async def _run_task_logic(
        self, context: RequestContext, updater: TaskUpdater, cancel_event: asyncio.Event, started_at: float
    ) -> None:
        task = context.current_task
        assert task is not None

        payload = _extract_payload(context)
        cfg = _load_cfg(payload.get("config") if isinstance(payload, dict) else {})

        out_dir = Path(payload.get("out_dir") or "results/a2a")
        repo_root = Path(__file__).resolve().parents[2]
        out_dir = (repo_root / out_dir) if not out_dir.is_absolute() else out_dir
        out_dir.mkdir(parents=True, exist_ok=True)
        run_id = cfg.run_id or f"a2a_phase2_{int(time.time())}"

        out_json = out_dir / f"{run_id}.json"
        log_dir = repo_root / "logs"
        events_path = log_dir / run_id / "events.jsonl"
        run_metadata_path = log_dir / run_id / "run_metadata.json"

        args = [
            "uv",
            "run",
            "smi-inhabit",
            "--corpus-root",
            cfg.corpus_root,
            "--package-ids-file",
            cfg.package_ids_file,
            "--agent",
            cfg.agent,
            "--rpc-url",
            cfg.rpc_url,
            "--simulation-mode",
            cfg.simulation_mode,
            "--per-package-timeout-seconds",
            str(cfg.per_package_timeout_seconds),
            "--max-plan-attempts",
            str(cfg.max_plan_attempts),
        ]
        if "max_planning_calls" in payload.get("config", {}):
            args.extend(["--max-planning-calls", str(int(payload["config"]["max_planning_calls"]))])

        args.extend(
            [
                "--out",
                str(out_json),
                "--run-id",
                run_id,
            ]
        )
        if cfg.samples and cfg.samples > 0:
            args.extend(["--samples", str(cfg.samples)])
        if cfg.continue_on_error:
            args.append("--continue-on-error")
        if cfg.resume:
            args.append("--resume")

        # Sanitize environment to prevent accidental bleed-through
        # while preserving essential system paths and SMI configuration.
        # We must pass enough for 'uv' and the shell to function.
        allowed_prefixes = (
            "SMI_",
            "RUST_",
            "CARGO_",
            "PATH",
            "HOME",
            "LANG",
            "LC_",
            "TERM",
            "UV_",
            "PYTHON",
            "OPENAI_",
            "OPENROUTER_",
            "ZAI_",
            "ZHIPUAI_",
        )
        sub_env = {k: v for k, v in os.environ.items() if any(k.startswith(p) for p in allowed_prefixes)}

        proc = await asyncio.create_subprocess_exec(
            *args,
            cwd=str(repo_root),
            env=sub_env,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
        )

        # Store process for cancellation support
        self._task_processes[task.id] = proc

        assert proc.stdout is not None
        proc_lines: collections.deque[str] = collections.deque(maxlen=2000)

        # Monitor both stdout and cancellation event
        try:
            async for b in proc.stdout:
                # Check for cancellation
                if cancel_event.is_set():
                    await updater.update_status(
                        TaskState.working,
                        new_agent_text_message(
                            "Cancellation requested, terminating process...",
                            updater.context_id,
                            updater.task_id,
                        ),
                    )
                    break

                line = b.decode("utf-8", errors="replace").rstrip("\n")
                if not line:
                    continue

                if line.startswith("A2A_EVENT:"):
                    # Structured event - update status
                    try:
                        raw_event = line[10:]
                        evt = safe_json_loads(raw_event, context="A2A event stream")
                        # Format a nice status message
                        msg = f"{evt.get('event', 'update')}"
                        if "package_id" in evt:
                            msg += f": {evt['package_id']}"
                        if "error" in evt and evt["error"]:
                            msg += f" (error: {evt['error']})"
                        elif "created_hits" in evt:
                            msg += f" (hits: {evt['created_hits']}/{evt.get('targets')})"

                        await updater.update_status(
                            TaskState.working,
                            new_agent_text_message(msg, updater.context_id, updater.task_id),
                        )
                    except Exception:
                        pass
                else:
                    # Normal log line - buffer for tail report but don't spam status
                    proc_lines.append(line)
        except asyncio.CancelledError:
            # Task was cancelled externally
            cancel_event.set()
            raise

        # If cancelled, handle graceful termination
        if cancel_event.is_set():
            await self._terminate_process(proc, updater)
            rc = proc.returncode if proc.returncode is not None else -1
        else:
            rc = await proc.wait()

        # Clean up process tracking
        self._task_processes.pop(task.id, None)
        self._task_cancel_events.pop(task.id, None)

        finished_at = time.time()

        # If task was cancelled, update status and return
        if cancel_event.is_set():
            await updater.update_status(
                TaskState.canceled,
                new_agent_text_message(
                    f"Task cancelled (exit_code={rc})",
                    updater.context_id,
                    updater.task_id,
                ),
            )
            return

        metrics: dict[str, Any] = {}
        errors_list: list[dict[str, Any]] = []
        if out_json.exists():
            metrics, errors_list = _summarize_phase2_results(out_json)

        # Defensive: if summary unexpectedly returned empty, re-parse from disk.
        if out_json.exists() and not metrics:
            parsed = _read_json(out_json)
            if parsed is not None:
                metrics, errors_list = _summarize_phase2_results(out_json)

        if out_json.exists() and not metrics:
            await updater.update_status(
                TaskState.working,
                new_agent_text_message(
                    "warning: phase2 results found but summary returned empty metrics",
                    updater.context_id,
                    updater.task_id,
                ),
            )

        # If the Phase II output is missing aggregate/package details for any reason,
        # fall back to deriving errors/metrics from the JSONL logs.
        if not metrics.get("errors") and run_metadata_path.exists():
            md = _read_json(run_metadata_path) or {}
            metrics.setdefault("run_metadata", md)

        # Add high-level failure mode analysis if errors occurred
        if errors_list:
            failure_summary = _summarize_failure_modes(errors_list)
            metrics["failure_modes"] = failure_summary

        bundle = {
            "schema_version": 1,
            "spec_url": "smi-bench:evaluation_bundle:v1",
            "benchmark": "phase2_inhabit",
            "run_id": run_id,
            "exit_code": rc,
            "timings": {
                "started_at_unix_seconds": int(started_at),
                "finished_at_unix_seconds": int(finished_at),
                "elapsed_seconds": finished_at - started_at,
            },
            "config": {
                "corpus_root": cfg.corpus_root,
                "package_ids_file": cfg.package_ids_file,
                "samples": cfg.samples,
                "rpc_url": cfg.rpc_url,
                "simulation_mode": cfg.simulation_mode,
                "per_package_timeout_seconds": cfg.per_package_timeout_seconds,
                "max_plan_attempts": cfg.max_plan_attempts,
                "continue_on_error": cfg.continue_on_error,
                "resume": cfg.resume,
            },
            "metrics": metrics,
            "errors": errors_list,
            "runner_output_tail": "\n".join(list(proc_lines)[-200:]),
            "artifacts": {
                "results_path": str(out_json),
                "run_metadata_path": str(run_metadata_path),
                "events_path": str(events_path),
            },
        }

        await updater.add_artifact(
            [Part(root=TextPart(text=json.dumps(bundle, sort_keys=True)))],
            name="evaluation_bundle",
        )
        if out_json.exists():
            await updater.add_artifact(
                [Part(root=TextPart(text=out_json.read_text(encoding="utf-8")))],
                name="phase2_results.json",
            )
        if run_metadata_path.exists():
            await updater.add_artifact(
                [Part(root=TextPart(text=run_metadata_path.read_text(encoding="utf-8")))],
                name="run_metadata.json",
            )

        if rc == 0:
            final_msg = "run_finished"
            if metrics.get("failure_modes"):
                final_msg += f" (failures: {metrics['failure_modes']})"
            await updater.complete(new_agent_text_message(final_msg, task.context_id, task.id))
        else:
            await updater.failed(
                new_agent_text_message(
                    f"phase2 failed (exit={rc})",
                    updater.context_id,
                    updater.task_id,
                )
            )

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        """
        Cancel a running task by gracefully terminating its subprocess.
        Implements A2A protocol cancellation support.
        """
        task = context.current_task
        if task is None:
            raise ValueError("No current task to cancel")

        # Check if task is in a terminal state
        if task.status in [TaskState.completed, TaskState.failed, TaskState.canceled, TaskState.rejected]:
            raise TaskNotCancelableError(task.id, getattr(task.status, "value", str(task.status)))

        # Signal cancellation to the execute() coroutine
        cancel_event = self._task_cancel_events.get(task.id)
        if cancel_event:
            cancel_event.set()

        proc = self._task_processes.get(task.id)
        if proc and proc.returncode is None:
            updater = TaskUpdater(event_queue, task.id, task.context_id)
            await self._terminate_process(proc, updater)

    async def _terminate_process(self, proc: asyncio.subprocess.Process, updater: TaskUpdater) -> None:
        """
        Gracefully terminate a subprocess using SIGTERM → SIGKILL pattern.
        """
        if proc.returncode is not None:
            return  # Already terminated

        try:
            # Send SIGTERM for graceful shutdown
            maybe_awaitable = proc.terminate()
            if inspect.isawaitable(maybe_awaitable):
                await maybe_awaitable
            await updater.update_status(
                TaskState.working,
                new_agent_text_message(
                    "Sent SIGTERM, waiting for graceful shutdown...",
                    updater.context_id,
                    updater.task_id,
                ),
            )

            # Wait up to 5 seconds for graceful termination
            try:
                await asyncio.wait_for(proc.wait(), timeout=5.0)
            except asyncio.TimeoutError:
                # Force kill if still running
                maybe_awaitable = proc.kill()
                if inspect.isawaitable(maybe_awaitable):
                    await maybe_awaitable
                await updater.update_status(
                    TaskState.working,
                    new_agent_text_message(
                        "Graceful shutdown timed out, sent SIGKILL",
                        updater.context_id,
                        updater.task_id,
                    ),
                )
                await proc.wait()
        except ProcessLookupError:
            # Process already died
            pass


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
    card = _card(url=public_url)
    handler = DefaultRequestHandler(
        agent_executor=SmiBenchGreenExecutor(),
        task_store=InMemoryTaskStore(),
    )
    app = A2AStarletteApplication(agent_card=card, http_handler=handler).build()

    # Add health check endpoint
    @app.route("/health")
    async def health(request: Request) -> JSONResponse:
        from smi_bench.inhabit_runner import _default_dev_inspect_binary
        from smi_bench.rust import default_rust_binary

        # Check binaries
        rust_bin = default_rust_binary()
        sim_bin = _default_dev_inspect_binary()

        status = {
            "status": "ok",
            "binaries": {
                "extractor": {"path": str(rust_bin), "exists": rust_bin.exists()},
                "simulator": {"path": str(sim_bin), "exists": sim_bin.exists()},
            },
        }

        # If binaries are missing, the agent is technically 'unhealthy' for its task
        if not (rust_bin.exists() and sim_bin.exists()):
            return JSONResponse(status, status_code=503)

        return JSONResponse(status)

    # Add A2A version header middleware
    app.add_middleware(cast(Any, A2AVersionMiddleware))

    return app


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(description="A2A green agent server for smi-bench Phase II")
    p.add_argument("--host", type=str, default="0.0.0.0")
    p.add_argument("--port", type=int, default=9999)
    p.add_argument("--card-url", type=str, default=None)
    args = p.parse_args(argv)

    url = args.card_url or f"http://{args.host}:{args.port}/"
    app = build_app(public_url=url)
    uvicorn.run(app, host=args.host, port=args.port)


if __name__ == "__main__":
    main()

