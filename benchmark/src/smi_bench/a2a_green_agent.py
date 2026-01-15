from __future__ import annotations

import argparse
import asyncio
import collections
import json
import logging
import os
import re
import signal
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, cast

import httpx
import uvicorn
from a2a.server.agent_execution import AgentExecutor, RequestContext
from a2a.server.apps import A2AStarletteApplication
from a2a.server.events import EventQueue
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore, TaskUpdater
from a2a.types import AgentCapabilities, AgentCard, AgentProvider, AgentSkill, Part, TaskState, TextPart
from a2a.utils import new_agent_text_message, new_task
from prometheus_client import CONTENT_TYPE_LATEST, Counter, Gauge, Histogram, generate_latest
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import JSONResponse, Response

from smi_bench.a2a_errors import A2AError, InvalidConfigError, TaskNotCancelableError
from smi_bench.constants import (
    DEFAULT_RPC_URL,
    HEALTH_CHECK_TIMEOUT_SECONDS,
)
from smi_bench.logging import default_run_id
from smi_bench.schema import Phase2ResultKeys
from smi_bench.utils import (
    managed_subprocess,
    safe_bool,
    safe_json_loads,
    safe_parse_int,
    validate_range,
)

logger = logging.getLogger(__name__)

# A2A Protocol version this implementation supports
A2A_PROTOCOL_VERSION = "0.3.0"

# Supported content types (for future content validation)
SUPPORTED_CONTENT_TYPES = {"application/json", "text/plain"}

# Environment variable for max concurrent tasks
MAX_CONCURRENT_TASKS_ENV = "SMI_MAX_CONCURRENT_TASKS"
DEFAULT_MAX_CONCURRENT_TASKS = 1

# Known config fields for strict validation
KNOWN_CONFIG_FIELDS = frozenset(
    {
        "corpus_root",
        "package_ids_file",
        "manifest",  # manifest is alias
        "samples",
        "agent",
        "rpc_url",
        "simulation_mode",
        "per_package_timeout_seconds",
        "max_plan_attempts",
        "continue_on_error",
        "resume",
        "run_id",
        "model",
        # P0 fields
        "seed",
        "sender",
        "gas_budget",
        "gas_coin",
        "gas_budget_ladder",
        "max_errors",
        "max_run_seconds",
        # P1 fields
        "max_planning_calls",
        "checkpoint_every",
        "max_heuristic_variants",
        "baseline_max_candidates",
        "include_created_types",
        "require_dry_run",
        # Webhook/async
        "callback_url",
        # Meta fields
        "out_dir",
    }
)

# Prometheus metrics
TASK_REQUESTS = Counter(
    "smi_bench_task_requests_total",
    "Total task requests received",
    ["agent_type", "simulation_mode"],
)
TASK_DURATION = Histogram(
    "smi_bench_task_duration_seconds",
    "Task execution duration in seconds",
    ["agent_type", "simulation_mode", "status"],
    buckets=(5, 10, 30, 60, 120, 300, 600, 1800, 3600),
)
TASK_ERRORS = Counter(
    "smi_bench_task_errors_total",
    "Total task errors by type",
    ["error_type"],
)
ACTIVE_TASKS = Gauge(
    "smi_bench_active_tasks",
    "Number of currently active tasks",
)
CONFIG_VALIDATION_REQUESTS = Counter(
    "smi_bench_config_validation_requests_total",
    "Config validation requests",
    ["valid"],
)
PACKAGES_PROCESSED = Counter(
    "smi_bench_packages_processed_total",
    "Total packages processed",
    ["agent_type", "result"],
)
HTTP_REQUESTS = Counter(
    "smi_bench_http_requests_total",
    "HTTP requests by endpoint and status",
    ["method", "endpoint", "status_code"],
)
HTTP_REQUEST_DURATION = Histogram(
    "smi_bench_http_request_duration_seconds",
    "HTTP request duration",
    ["method", "endpoint"],
    buckets=(0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0),
)


@dataclass(frozen=True)
class EvalConfig:
    # Core required fields
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
    # Optional with defaults - P0 critical
    model: str | None = None
    seed: int = 0
    sender: str | None = None
    gas_budget: int = 10_000_000
    gas_coin: str | None = None
    gas_budget_ladder: str = "20000000,50000000"
    max_errors: int = 25
    max_run_seconds: float | None = None
    # P1 flexibility
    max_planning_calls: int = 50
    checkpoint_every: int = 10
    max_heuristic_variants: int = 4
    baseline_max_candidates: int = 25
    include_created_types: bool = False
    require_dry_run: bool = False
    # Webhook/async support
    callback_url: str | None = None


def _load_cfg(raw: Any) -> EvalConfig:
    if not isinstance(raw, dict):
        raise InvalidConfigError("config", "must be a JSON object")

    corpus_root = str(raw.get("corpus_root") or "")
    package_ids_file = str(raw.get("package_ids_file") or raw.get("manifest") or "")

    if not corpus_root:
        raise InvalidConfigError("corpus_root", "missing or empty")
    if "\0" in corpus_root:
        raise InvalidConfigError("corpus_root", "path contains null bytes")

    if not package_ids_file:
        raise InvalidConfigError("package_ids_file", "missing or empty")
    if "\0" in package_ids_file:
        raise InvalidConfigError("package_ids_file", "path contains null bytes")

    # Early check: if we are running locally (not a purely dry-run metadata check),
    # the manifest should exist.
    manifest_path = Path(package_ids_file)
    if not manifest_path.is_file():
        # Fallback for relative paths if repo_root is available
        repo_root = Path(__file__).resolve().parents[2]
        if not (repo_root / manifest_path).is_file():
            raise InvalidConfigError("package_ids_file", f"manifest file not found: {package_ids_file}")

    # Fail-fast validation: simulation modes that require sender
    simulation_mode = str(raw.get("simulation_mode") or "dry-run")
    sender_raw = raw.get("sender")
    sender = str(sender_raw) if sender_raw else None
    if simulation_mode in ("dev-inspect", "execute") and not sender:
        raise InvalidConfigError(
            "sender",
            f"required for simulation_mode={simulation_mode} (provide a valid Sui address)",
        )

    # Basic Sui address format validation if sender is provided
    if sender:
        # Must be 0x followed by 1-64 hex chars
        if not re.match(r"^0x[0-9a-fA-F]{1,64}$", sender):
            raise InvalidConfigError("sender", f"invalid Sui address format: {sender}")

    # Validate model if provided
    model_raw = raw.get("model")
    if model_raw is not None:
        model = str(model_raw).strip()
        if not model:
            raise InvalidConfigError("model", "must not be an empty string if provided")
    else:
        model = None

    # P0: Parse critical production fields
    seed = safe_parse_int(raw.get("seed"), 0, min_val=0, name="seed")

    try:
        gas_budget = int(validate_range(raw.get("gas_budget", 10_000_000), min_val=1_000_000, name="gas_budget"))
    except ValueError as e:
        raise InvalidConfigError("gas_budget", str(e))

    gas_coin_raw = raw.get("gas_coin")
    gas_coin = str(gas_coin_raw) if gas_coin_raw else None

    gas_budget_ladder = str(raw.get("gas_budget_ladder") or "20000000,50000000")

    try:
        max_errors = int(validate_range(raw.get("max_errors", 25), min_val=1, name="max_errors"))
    except ValueError as e:
        raise InvalidConfigError("max_errors", str(e))

    max_run_seconds_raw = raw.get("max_run_seconds")
    max_run_seconds: float | None = None
    if max_run_seconds_raw is not None:
        try:
            max_run_seconds = validate_range(max_run_seconds_raw, min_val=1.0, name="max_run_seconds")
        except ValueError as e:
            raise InvalidConfigError("max_run_seconds", str(e))

    # P1: Parse flexibility fields
    try:
        max_planning_calls = int(
            validate_range(raw.get("max_planning_calls", 50), min_val=1, name="max_planning_calls")
        )
        checkpoint_every = int(validate_range(raw.get("checkpoint_every", 10), min_val=1, name="checkpoint_every"))
        max_heuristic_variants = int(
            validate_range(raw.get("max_heuristic_variants", 4), min_val=1, name="max_heuristic_variants")
        )
        baseline_max_candidates = int(
            validate_range(raw.get("baseline_max_candidates", 25), min_val=1, name="baseline_max_candidates")
        )
    except ValueError as e:
        raise InvalidConfigError("range_validation", str(e))

    include_created_types = safe_bool(raw.get("include_created_types"), False)
    require_dry_run = safe_bool(raw.get("require_dry_run"), False)

    # Webhook/async support
    callback_url_raw = raw.get("callback_url")
    callback_url = str(callback_url_raw) if callback_url_raw else None

    # Cross-field validation
    if require_dry_run and simulation_mode != "dry-run":
        raise InvalidConfigError(
            "require_dry_run",
            "can only be true when simulation_mode is 'dry-run'",
        )

    try:
        per_package_timeout_seconds = validate_range(
            raw.get("per_package_timeout_seconds", 300.0), min_val=1.0, name="per_package_timeout_seconds"
        )
        max_plan_attempts = int(validate_range(raw.get("max_plan_attempts", 2), min_val=1, name="max_plan_attempts"))
    except ValueError as e:
        raise InvalidConfigError("timeout_or_attempts", str(e))

    return EvalConfig(
        corpus_root=corpus_root,
        package_ids_file=package_ids_file,
        samples=safe_parse_int(raw.get("samples"), 0, min_val=0, name="samples"),
        agent=str(raw.get("agent") or "real-openai-compatible"),
        rpc_url=str(raw.get("rpc_url") or DEFAULT_RPC_URL),
        simulation_mode=simulation_mode,
        per_package_timeout_seconds=per_package_timeout_seconds,
        max_plan_attempts=max_plan_attempts,
        continue_on_error=safe_bool(raw.get("continue_on_error"), True),
        resume=safe_bool(raw.get("resume"), True),
        run_id=str(raw.get("run_id")) if raw.get("run_id") else None,
        model=model,
        # P0 fields
        seed=seed,
        sender=sender,
        gas_budget=gas_budget,
        gas_coin=gas_coin,
        gas_budget_ladder=gas_budget_ladder,
        max_errors=max_errors,
        max_run_seconds=max_run_seconds,
        # P1 fields
        max_planning_calls=max_planning_calls,
        checkpoint_every=checkpoint_every,
        max_heuristic_variants=max_heuristic_variants,
        baseline_max_candidates=baseline_max_candidates,
        include_created_types=include_created_types,
        require_dry_run=require_dry_run,
        callback_url=callback_url,
    )


async def _send_webhook_callback(url: str, payload: dict[str, Any]) -> None:
    """
    Send task completion results to webhook URL.

    Non-blocking, fire-and-forget with basic error logging.
    """
    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            response = await client.post(
                url,
                json=payload,
                headers={"Content-Type": "application/json"},
            )
            if response.status_code >= 400:
                logger.warning(f"Webhook callback failed: {url} returned {response.status_code}")
            else:
                logger.info(f"Webhook callback successful: {url}")
    except Exception as e:
        logger.error(f"Webhook callback error to {url}: {e}", exc_info=True)


def _detect_unknown_fields(raw: dict[str, Any]) -> list[str]:
    """Return list of unknown field names in the config."""
    if not isinstance(raw, dict):
        return []
    return [k for k in raw if k not in KNOWN_CONFIG_FIELDS]


def _validate_config_dry_run(raw: Any) -> dict[str, Any]:
    """
    Validate config without executing. Returns validation result.

    Returns dict with:
        - valid: bool
        - config: parsed EvalConfig as dict (if valid)
        - error: error message (if invalid)
        - warnings: list of warnings (e.g., unknown fields)
    """
    result: dict[str, Any] = {"valid": False, "warnings": []}

    if not isinstance(raw, dict):
        result["error"] = "config must be a JSON object"
        return result

    # Check for unknown fields
    unknown = _detect_unknown_fields(raw)
    if unknown:
        result["warnings"].append(f"Unknown config fields (will be ignored): {unknown}")

    # Try to parse
    try:
        cfg = _load_cfg(raw)
        result["valid"] = True
        result["config"] = {
            "corpus_root": cfg.corpus_root,
            "package_ids_file": cfg.package_ids_file,
            "samples": cfg.samples,
            "agent": cfg.agent,
            "rpc_url": cfg.rpc_url,
            "simulation_mode": cfg.simulation_mode,
            "per_package_timeout_seconds": cfg.per_package_timeout_seconds,
            "max_plan_attempts": cfg.max_plan_attempts,
            "continue_on_error": cfg.continue_on_error,
            "resume": cfg.resume,
            "run_id": cfg.run_id,
            "model": cfg.model,
            "seed": cfg.seed,
            "sender": cfg.sender,
            "gas_budget": cfg.gas_budget,
            "gas_coin": cfg.gas_coin,
            "gas_budget_ladder": cfg.gas_budget_ladder,
            "max_errors": cfg.max_errors,
            "max_run_seconds": cfg.max_run_seconds,
            "max_planning_calls": cfg.max_planning_calls,
            "checkpoint_every": cfg.checkpoint_every,
            "max_heuristic_variants": cfg.max_heuristic_variants,
            "baseline_max_candidates": cfg.baseline_max_candidates,
            "include_created_types": cfg.include_created_types,
            "require_dry_run": cfg.require_dry_run,
            "callback_url": cfg.callback_url,
        }
    except InvalidConfigError as e:
        result["error"] = str(e)
    except Exception as e:
        result["error"] = f"Unexpected error: {type(e).__name__}: {e}"

    return result


def _get_config_schema() -> dict[str, Any]:
    """Return JSON Schema for EvalConfig."""
    return {
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "EvalConfig",
        "description": "Configuration for SMI Bench Phase II evaluation task",
        "type": "object",
        "required": ["corpus_root", "package_ids_file"],
        "properties": {
            "corpus_root": {
                "type": "string",
                "description": "Path to bytecode corpus directory",
                "minLength": 1,
            },
            "package_ids_file": {
                "type": "string",
                "description": "Path to manifest file (one package ID per line)",
                "minLength": 1,
            },
            "samples": {
                "type": "integer",
                "description": "Number of packages to process (0 = all)",
                "default": 0,
                "minimum": 0,
            },
            "agent": {
                "type": "string",
                "description": "Agent type to use",
                "default": "real-openai-compatible",
                "enum": ["mock-empty", "mock-planfile", "real-openai-compatible", "baseline-search", "template-search"],
            },
            "rpc_url": {
                "type": "string",
                "description": "Sui fullnode RPC URL for simulation",
                "default": DEFAULT_RPC_URL,
            },
            "simulation_mode": {
                "type": "string",
                "description": "Transaction simulation mode",
                "default": "dry-run",
                "enum": ["dry-run", "dev-inspect", "build-only"],
            },
            "per_package_timeout_seconds": {
                "type": "number",
                "description": "Wall-clock budget per package in seconds",
                "default": 300.0,
                "minimum": 1.0,
            },
            "max_plan_attempts": {
                "type": "integer",
                "description": "Max PTB replanning attempts per package",
                "default": 2,
                "minimum": 1,
            },
            "continue_on_error": {
                "type": "boolean",
                "description": "Continue benchmark if a package fails",
                "default": True,
            },
            "resume": {
                "type": "boolean",
                "description": "Resume from existing output file",
                "default": True,
            },
            "run_id": {
                "type": ["string", "null"],
                "description": "Custom run identifier",
                "default": None,
            },
            "model": {
                "type": ["string", "null"],
                "description": "Per-request model override (takes precedence over SMI_MODEL env var)",
                "default": None,
            },
            "seed": {
                "type": "integer",
                "description": "Random seed for reproducible sampling",
                "default": 0,
                "minimum": 0,
            },
            "sender": {
                "type": ["string", "null"],
                "description": "Sui address for tx simulation (required for dev-inspect/execute modes)",
                "default": None,
                "pattern": "^0x[0-9a-fA-F]{1,64}$",
            },
            "gas_budget": {
                "type": "integer",
                "description": "Gas budget for dry-run simulation",
                "default": 10000000,
                "minimum": 1000000,
            },
            "gas_coin": {
                "type": ["string", "null"],
                "description": "Specific gas coin object ID",
                "default": None,
            },
            "gas_budget_ladder": {
                "type": "string",
                "description": "Comma-separated retry budgets on InsufficientGas",
                "default": "20000000,50000000",
            },
            "max_errors": {
                "type": "integer",
                "description": "Stop run after N package errors",
                "default": 25,
                "minimum": 1,
            },
            "max_run_seconds": {
                "type": ["number", "null"],
                "description": "Wall-clock budget for entire run in seconds",
                "default": None,
                "minimum": 1.0,
            },
            "max_planning_calls": {
                "type": "integer",
                "description": "Max LLM calls per package (progressive exposure)",
                "default": 50,
                "minimum": 1,
            },
            "checkpoint_every": {
                "type": "integer",
                "description": "Save partial results every N packages",
                "default": 10,
                "minimum": 1,
            },
            "max_heuristic_variants": {
                "type": "integer",
                "description": "Max deterministic PTB variants per plan attempt",
                "default": 4,
                "minimum": 1,
            },
            "baseline_max_candidates": {
                "type": "integer",
                "description": "Max candidates in baseline-search mode",
                "default": 25,
                "minimum": 1,
            },
            "include_created_types": {
                "type": "boolean",
                "description": "Include full created object type lists in output",
                "default": False,
            },
            "require_dry_run": {
                "type": "boolean",
                "description": "Fail if dry-run unavailable (no dev-inspect fallback)",
                "default": False,
            },
            "callback_url": {
                "type": ["string", "null"],
                "description": "HTTP URL to POST task results when completed (webhook callback for async workflows)",
                "default": None,
            },
        },
        "additionalProperties": False,
    }


def _extract_payload(context: RequestContext) -> dict[str, Any]:
    raw = context.get_user_input()
    if raw:
        try:
            v = safe_json_loads(raw, context="user input payload")
            if isinstance(v, dict):
                return v
        except (json.JSONDecodeError, TypeError, ValueError):
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
    except (OSError, json.JSONDecodeError, TypeError, ValueError) as e:
        logger.error("Failed to load phase2 results JSON: %s", e, exc_info=True)
        return ({}, [])
    # Return as much as we can even if some fields are missing.
    if not isinstance(data, dict):
        return ({}, [])

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
    except (OSError, json.JSONDecodeError, TypeError, ValueError) as e:
        logger.warning("Failed to read JSON file %s: %s", path, e, exc_info=True)
        return None
    if not isinstance(data, dict):
        return None
    return data


def _card(*, url: str) -> AgentCard:
    skill = AgentSkill(
        id="run_phase2",
        name="Run Phase II",
        description=(
            "Run Phase II (PTB inhabitation) over a manifest and return results as artifacts. "
            "Optional 'model' field in config allows per-request model override "
            "(takes precedence over SMI_MODEL environment variable)."
        ),
        tags=["benchmark", "sui", "move", "phase2"],
        examples=["Run Phase II with default model", "Run Phase II with specific model override"],
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


# Global reference for shutdown handling
_global_executor: SmiBenchGreenExecutor | None = None

# Global task results store for partial results endpoint
_task_results: dict[str, dict[str, Any]] = {}


class SmiBenchGreenExecutor(AgentExecutor):
    def __init__(self) -> None:
        super().__init__()
        # Track running subprocesses for cancellation support
        self._task_processes: dict[str, asyncio.subprocess.Process] = {}
        self._task_cancel_events: dict[str, asyncio.Event] = {}
        # Limit concurrency to prevent OOM and RPC stampedes (configurable via env var)
        self._max_concurrent_tasks = safe_parse_int(
            os.environ.get(MAX_CONCURRENT_TASKS_ENV), DEFAULT_MAX_CONCURRENT_TASKS, min_val=1
        )
        self._concurrency_semaphore: asyncio.Semaphore | None = None
        logger.info(f"Max concurrent tasks: {self._max_concurrent_tasks} (set {MAX_CONCURRENT_TASKS_ENV} to change)")

        # Register this instance globally for signal/shutdown handling
        global _global_executor
        _global_executor = self

    async def shutdown(self) -> None:
        """
        Gracefully terminate all running task processes.
        Called on server shutdown (SIGINT/SIGTERM).
        """
        active_task_ids = list(self._task_processes.keys())
        if not active_task_ids:
            return

        print(f"Shutting down {len(active_task_ids)} active benchmark tasks...")

        # Send termination signals to all
        termination_tasks = []
        for task_id in active_task_ids:
            proc = self._task_processes.get(task_id)
            if proc and proc.returncode is None:
                # We don't have a TaskUpdater here so we use a mock-like termination
                try:
                    proc.terminate()
                    termination_tasks.append(asyncio.wait_for(proc.wait(), timeout=2.0))
                except Exception:
                    pass

        if termination_tasks:
            await asyncio.gather(*termination_tasks, return_exceptions=True)

        # Force kill any survivors
        for task_id in active_task_ids:
            proc = self._task_processes.get(task_id)
            if proc and proc.returncode is None:
                try:
                    proc.kill()
                except Exception:
                    pass

        self._task_processes.clear()
        print("All benchmark tasks terminated.")

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        if self._concurrency_semaphore is None:
            self._concurrency_semaphore = asyncio.Semaphore(self._max_concurrent_tasks)

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
            ACTIVE_TASKS.inc()
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
                    ACTIVE_TASKS.dec()
                    return
                except A2AError as e:
                    # Specific protocol errors should not be retried, but must be reported
                    ACTIVE_TASKS.dec()
                    TASK_ERRORS.labels(error_type="a2a_error").inc()
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
                    ACTIVE_TASKS.dec()
                    error_type = "infra_error" if is_infra_failure else "unexpected_error"
                    TASK_ERRORS.labels(error_type=error_type).inc()
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

        repo_root = Path(__file__).resolve().parents[2]
        payload = _extract_payload(context)
        config = payload.get("config") if isinstance(payload.get("config"), dict) else {}
        cfg = _load_cfg(config)

        # Record task request metrics
        TASK_REQUESTS.labels(agent_type=cfg.agent, simulation_mode=cfg.simulation_mode).inc()

        # Initialize task results store
        _task_results[task.id] = {
            "task_id": task.id,
            "status": "running",
            "started_at": int(started_at),
            "agent": cfg.agent,
            "simulation_mode": cfg.simulation_mode,
            "run_id": None,  # Will be set once known
            "partial_metrics": {},
            "error": None,
        }

        # Default manifest path
        default_manifest = repo_root / "manifests/datasets/type_inhabitation_top25.txt"
        package_ids_file = Path(cfg.package_ids_file) if cfg.package_ids_file else default_manifest
        if not package_ids_file.is_absolute():
            package_ids_file = repo_root / package_ids_file

        out_dir = Path(payload.get("out_dir") or "results/a2a")
        out_dir = (repo_root / out_dir) if not out_dir.is_absolute() else out_dir
        out_dir.mkdir(parents=True, exist_ok=True)
        run_id = cfg.run_id or default_run_id(prefix="a2a_phase2")

        out_json = out_dir / f"{run_id}.json"
        log_dir = repo_root / "logs"
        events_path = log_dir / run_id / "events.jsonl"
        run_metadata_path = log_dir / run_id / "run_metadata.json"

        venv_bin = repo_root / ".venv" / "bin"

        args = [
            str(venv_bin / "smi-inhabit"),
            "--corpus-root",
            str(cfg.corpus_root),
            "--package-ids-file",
            str(package_ids_file),
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
            "--out",
            str(out_json),
            "--run-id",
            run_id,
            "--samples",
            str(cfg.samples),
            # P0: Always pass critical production fields
            "--seed",
            str(cfg.seed),
            "--gas-budget",
            str(cfg.gas_budget),
            "--gas-budget-ladder",
            cfg.gas_budget_ladder,
            "--max-errors",
            str(cfg.max_errors),
            # P1: Always pass flexibility fields
            "--max-planning-calls",
            str(cfg.max_planning_calls),
            "--checkpoint-every",
            str(cfg.checkpoint_every),
            "--max-heuristic-variants",
            str(cfg.max_heuristic_variants),
            "--baseline-max-candidates",
            str(cfg.baseline_max_candidates),
        ]

        # Conditional flags
        if cfg.continue_on_error:
            args.append("--continue-on-error")
        if cfg.sender:
            args.extend(["--sender", cfg.sender])
        if cfg.gas_coin:
            args.extend(["--gas-coin", cfg.gas_coin])
        if cfg.max_run_seconds is not None:
            args.extend(["--max-run-seconds", str(cfg.max_run_seconds)])
        if cfg.include_created_types:
            args.append("--include-created-types")
        if cfg.require_dry_run:
            args.append("--require-dry-run")

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

        # Override SMI_MODEL if provided in config
        if cfg.model:
            sub_env["SMI_MODEL"] = cfg.model
            logger.info(f"Model override active: SMI_MODEL={cfg.model}")

        async with managed_subprocess(
            *args,
            cwd=str(repo_root),
            env=sub_env,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
        ) as proc:
            # Store process for cancellation support
            self._task_processes[task.id] = proc

            assert proc.stdout is not None
            # P2: Reduced buffer from 2000 to 500 lines to limit memory under high concurrency
            # 500 lines * ~200 chars = ~100KB per task (vs 400KB previously)
            proc_lines: collections.deque[str] = collections.deque(maxlen=500)

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
                            if evt.get("error"):
                                msg += f" (error: {evt['error']})"
                            elif "created_hits" in evt:
                                msg += f" (hits: {evt['created_hits']}/{evt.get('targets')})"

                            await updater.update_status(
                                TaskState.working,
                                new_agent_text_message(msg, updater.context_id, updater.task_id),
                            )
                        except Exception as e:
                            logger.debug("Failed to update task status: %s", e)
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
            metrics["run_metadata"] = md

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

        # Record task duration and result
        duration = time.time() - started_at

        # Update task results store
        final_status = "completed" if rc == 0 else "failed"
        _task_results[task.id].update(
            {
                "status": final_status,
                "completed_at": int(time.time()),
                "duration_seconds": duration,
                "run_id": run_id,
                "bundle": bundle,
                "metrics": metrics,
                "error_summary": errors_list[:10] if errors_list else [],
            }
        )

        # Prepare webhook payload if callback_url is configured
        if cfg.callback_url:
            webhook_payload = {
                "task_id": task.id,
                "status": final_status,
                "duration_seconds": duration,
                "run_id": run_id,
                "agent": cfg.agent,
                "simulation_mode": cfg.simulation_mode,
                "bundle": bundle,
                "metrics": metrics,
                "error_summary": errors_list[:10] if errors_list else [],  # Limit to first 10 errors
                "timestamp": int(time.time()),
            }

        if rc == 0:
            TASK_DURATION.labels(agent_type=cfg.agent, simulation_mode=cfg.simulation_mode, status="success").observe(
                duration
            )
            final_msg = "run_finished"
            if metrics.get("failure_modes"):
                final_msg += f" (failures: {metrics['failure_modes']})"
            await updater.complete(new_agent_text_message(final_msg, task.context_id, task.id))

            # Send webhook callback asynchronously (fire and forget)
            if cfg.callback_url:
                asyncio.create_task(_send_webhook_callback(cfg.callback_url, webhook_payload))
        else:
            TASK_DURATION.labels(agent_type=cfg.agent, simulation_mode=cfg.simulation_mode, status="failed").observe(
                duration
            )
            TASK_ERRORS.labels(error_type="subprocess_failure").inc()
            await updater.failed(
                new_agent_text_message(
                    f"phase2 failed (exit={rc})",
                    updater.context_id,
                    updater.task_id,
                )
            )

            # Send webhook callback asynchronously (fire and forget)
            if cfg.callback_url:
                asyncio.create_task(_send_webhook_callback(cfg.callback_url, webhook_payload))

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
            return

        try:
            proc.terminate()

            await updater.update_status(
                TaskState.working,
                new_agent_text_message(
                    "Sent SIGTERM, waiting for graceful shutdown...",
                    updater.context_id,
                    updater.task_id,
                ),
            )

            try:
                await asyncio.wait_for(proc.wait(), timeout=5.0)
            except TimeoutError:
                try:
                    proc.kill()
                except ProcessLookupError:
                    pass
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
            logger.debug("Process already terminated")
        except Exception as e:
            logger.warning(f"Error terminating process: {e}")


class A2AVersionMiddleware(BaseHTTPMiddleware):
    """
    Middleware to add A2A-Version header to all responses.
    Implements A2A protocol version signaling per spec section 14.2.1.
    """

    async def dispatch(self, request: Request, call_next: Any) -> Response:
        response = await call_next(request)
        response.headers["A2A-Version"] = A2A_PROTOCOL_VERSION
        return response


class MetricsMiddleware(BaseHTTPMiddleware):
    """
    Middleware to track HTTP request metrics.
    Records request count, duration, and status codes per endpoint.
    """

    async def dispatch(self, request: Request, call_next: Any) -> Response:
        # Normalize endpoint path for metrics
        path = request.url.path
        if path.startswith("/.well-known/"):
            endpoint = "/.well-known/agent-card"
        elif path == "/":
            endpoint = "/rpc"
        else:
            endpoint = path

        method = request.method
        start_time = time.time()

        try:
            response = await call_next(request)
            status_code = response.status_code
        except Exception:
            status_code = 500
            HTTP_REQUESTS.labels(method=method, endpoint=endpoint, status_code=status_code).inc()
            raise
        else:
            HTTP_REQUESTS.labels(method=method, endpoint=endpoint, status_code=status_code).inc()
            return response
        finally:
            duration = time.time() - start_time
            HTTP_REQUEST_DURATION.labels(method=method, endpoint=endpoint).observe(duration)


def build_app(*, public_url: str) -> Any:
    card = _card(url=public_url)
    executor = SmiBenchGreenExecutor()
    handler = DefaultRequestHandler(
        agent_executor=executor,
        task_store=InMemoryTaskStore(),
    )
    app = A2AStarletteApplication(agent_card=card, http_handler=handler).build()  # type: ignore

    # Register shutdown handler
    @app.on_event("shutdown")
    async def shutdown_event():
        if _global_executor:
            await _global_executor.shutdown()

    # Add health check endpoint with comprehensive status
    @app.route("/health")
    async def health(request: Request) -> JSONResponse:
        from smi_bench.inhabit_runner import _default_dev_inspect_binary
        from smi_bench.rust import default_rust_binary

        # Check binaries
        rust_bin = default_rust_binary()
        sim_bin = _default_dev_inspect_binary()

        binaries_ok = rust_bin.exists() and sim_bin.exists()

        # Check RPC connectivity (non-blocking with timeout)
        rpc_url = DEFAULT_RPC_URL
        rpc_status = {"url": rpc_url, "reachable": False, "error": None}
        try:
            async with httpx.AsyncClient(timeout=HEALTH_CHECK_TIMEOUT_SECONDS) as client:
                resp = await client.post(
                    rpc_url,
                    json={
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "sui_getLatestCheckpointSequenceNumber",
                    },
                )
                if resp.status_code == 200:
                    data = resp.json()
                    if "result" in data:
                        rpc_status["reachable"] = True
                        rpc_status["checkpoint"] = data.get("result")
                    elif "error" in data:
                        rpc_status["error"] = str(data.get("error", {}).get("message", "unknown"))
                else:
                    rpc_status["error"] = f"HTTP {resp.status_code}"
        except httpx.TimeoutException:
            rpc_status["error"] = "timeout"
        except Exception as e:
            rpc_status["error"] = f"{type(e).__name__}: {e}"

        # Get executor status if available
        executor_status = {"active_tasks": 0, "task_ids": []}
        if _global_executor is not None:
            active_tasks = list(_global_executor._task_processes.keys())
            executor_status["active_tasks"] = len(active_tasks)
            executor_status["task_ids"] = active_tasks[:5]  # Limit to first 5

        # Determine overall health
        is_healthy = binaries_ok  # RPC is optional for health (might use different RPC per request)
        overall_status = "ok" if is_healthy else "degraded"

        status = {
            "status": overall_status,
            "binaries": {
                "extractor": {"path": str(rust_bin), "exists": rust_bin.exists()},
                "simulator": {"path": str(sim_bin), "exists": sim_bin.exists()},
            },
            "rpc": rpc_status,
            "executor": executor_status,
        }

        # Return 503 if binaries are missing
        if not binaries_ok:
            return JSONResponse(status, status_code=503)

        return JSONResponse(status)

    @app.route("/validate", methods=["POST"])
    async def validate_config(request: Request) -> JSONResponse:
        """
        Validate a config without executing a task.

        POST /validate
        Body: {"config": {...}}

        Returns:
            200: {"valid": true, "config": {...normalized...}, "warnings": [...]}
            400: {"valid": false, "error": "...", "warnings": [...]}
        """
        try:
            body = await request.json()
        except json.JSONDecodeError as e:
            return JSONResponse(
                {"valid": False, "error": f"Invalid JSON: {e}", "warnings": []},
                status_code=400,
            )

        config = body.get("config") if isinstance(body, dict) else None
        if config is None:
            return JSONResponse(
                {"valid": False, "error": "Missing 'config' field in request body", "warnings": []},
                status_code=400,
            )

        result = _validate_config_dry_run(config)
        status_code = 200 if result["valid"] else 400
        CONFIG_VALIDATION_REQUESTS.labels(valid=str(result["valid"]).lower()).inc()
        return JSONResponse(result, status_code=status_code)

    @app.route("/schema")
    async def get_schema(request: Request) -> JSONResponse:
        """
        Return JSON Schema for EvalConfig.

        GET /schema

        Returns:
            200: JSON Schema document
        """
        return JSONResponse(_get_config_schema())

    @app.route("/info")
    async def get_info(request: Request) -> JSONResponse:
        """
        Return API info including version, capabilities, and config limits.

        GET /info

        Returns server configuration and limits for client integration.
        """
        max_concurrent = DEFAULT_MAX_CONCURRENT_TASKS
        if _global_executor is not None:
            max_concurrent = _global_executor._max_concurrent_tasks

        return JSONResponse(
            {
                "version": "0.1.0",
                "a2a_protocol_version": A2A_PROTOCOL_VERSION,
                "capabilities": {
                    "streaming": True,
                    "cancellation": True,
                    "config_validation": True,
                    "schema_endpoint": True,
                },
                "limits": {
                    "max_concurrent_tasks": max_concurrent,
                },
                "endpoints": {
                    "health": "/health",
                    "validate": "/validate (POST)",
                    "schema": "/schema",
                    "info": "/info",
                    "task_results": "/tasks/{task_id}/results",
                    "metrics": "/metrics",
                    "agent_card": "/.well-known/agent-card.json",
                },
            }
        )

    @app.route("/tasks/{task_id}/results")
    async def get_task_results(request: Request) -> JSONResponse:
        """
        Get partial or complete results for a task.

        GET /tasks/{task_id}/results

        Returns:
            200: Task results (partial or complete)
            404: Task not found
        """
        task_id = request.path_params.get("task_id")
        if not task_id or task_id not in _task_results:
            return JSONResponse(
                {"error": f"Task {task_id} not found"},
                status_code=404,
            )

        return JSONResponse(_task_results[task_id])

    @app.route("/metrics")
    async def get_metrics(request: Request) -> Response:
        """
        Return Prometheus metrics.

        GET /metrics

        Returns:
            200: Prometheus text format metrics
        """
        return Response(generate_latest(), media_type=CONTENT_TYPE_LATEST)

    # Add middlewares (order matters: metrics first, then version)
    app.add_middleware(cast(Any, MetricsMiddleware))
    app.add_middleware(cast(Any, A2AVersionMiddleware))

    return app


# NOTE: _global_executor is declared at module level (line ~733) and should not be
# redeclared here. The global statement in main() is sufficient.


def _setup_signal_handlers() -> None:
    """
    Set up signal handlers for graceful shutdown.

    When SIGTERM/SIGINT is received, terminate all running subprocesses
    before exiting. This ensures no zombie processes when the container stops.
    """

    def handler(signum: int, frame: Any) -> None:
        if _global_executor is not None:
            for task_id, proc in list(_global_executor._task_processes.items()):
                if proc.returncode is None:
                    try:
                        proc.terminate()
                    except (OSError, ProcessLookupError) as e:
                        logger.debug("Failed to terminate process: %s", e)
        sys.exit(128 + signum)

    signal.signal(signal.SIGTERM, handler)
    signal.signal(signal.SIGINT, handler)


def main(argv: list[str] | None = None) -> None:
    global _global_executor

    # Configure logging to show INFO level
    logging.basicConfig(level=logging.INFO, format="%(asctime)s - %(name)s - %(levelname)s - %(message)s")

    p = argparse.ArgumentParser(description="A2A green agent server for smi-bench Phase II")
    p.add_argument("--host", type=str, default="0.0.0.0")
    p.add_argument("--port", type=int, default=9999)
    p.add_argument("--card-url", type=str, default=None)
    args = p.parse_args(argv)

    # Set up signal handlers before starting server
    _setup_signal_handlers()

    url = args.card_url or f"http://{args.host}:{args.port}/"
    app = build_app(public_url=url)
    uvicorn.run(app, host=args.host, port=args.port)


if __name__ == "__main__":
    main()
