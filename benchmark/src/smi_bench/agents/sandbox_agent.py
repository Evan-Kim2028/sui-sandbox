"""
Agentic LLM agent with full sandbox tool access.

This agent exposes all sandbox capabilities to the LLM via function calling,
allowing it to interactively explore packages, create objects, execute PTBs,
and iterate based on results.
"""

from __future__ import annotations

import json
import logging
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import httpx

from smi_bench.agents.real_agent import LLMUsage, RealAgentConfig, load_real_agent_config
from smi_bench.logging import JsonlLogger

logger = logging.getLogger(__name__)

# Maximum number of tool-use iterations before forcing final answer
MAX_ITERATIONS = 15

# ============================================================================
# Tool Definitions (OpenAI Function Calling Format)
# ============================================================================

SANDBOX_TOOLS = [
    {
        "type": "function",
        "function": {
            "name": "list_modules",
            "description": "List all loaded Move modules in the sandbox. Returns module paths like '0x2::coin'.",
            "parameters": {"type": "object", "properties": {}, "required": []},
        },
    },
    {
        "type": "function",
        "function": {
            "name": "get_struct_info",
            "description": "Get detailed information about a struct: fields, abilities, type parameters.",
            "parameters": {
                "type": "object",
                "properties": {
                    "module_path": {
                        "type": "string",
                        "description": "Module path like '0x2::coin' or full address",
                    },
                    "struct_name": {"type": "string", "description": "Name of the struct"},
                },
                "required": ["module_path", "struct_name"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "get_function_info",
            "description": "Get function signature: visibility, parameters, return types, type parameters.",
            "parameters": {
                "type": "object",
                "properties": {
                    "module_path": {"type": "string", "description": "Module path"},
                    "function_name": {"type": "string", "description": "Name of the function"},
                },
                "required": ["module_path", "function_name"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "list_functions",
            "description": "List all functions in a module with their signatures.",
            "parameters": {
                "type": "object",
                "properties": {
                    "module_path": {"type": "string", "description": "Module path like '0x2::coin'"},
                },
                "required": ["module_path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "list_structs",
            "description": "List all struct types defined in a module.",
            "parameters": {
                "type": "object",
                "properties": {
                    "module_path": {"type": "string", "description": "Module path"},
                },
                "required": ["module_path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "find_constructors",
            "description": "Find all functions that can construct/return a given type. Useful for understanding how to create objects.",
            "parameters": {
                "type": "object",
                "properties": {
                    "type_path": {
                        "type": "string",
                        "description": "Full type path like '0x2::coin::Coin' or '0xabc::module::MyStruct'",
                    },
                },
                "required": ["type_path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "search_types",
            "description": "Search for types matching a pattern across all loaded modules.",
            "parameters": {
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Pattern with * wildcard (e.g., '*Coin*', '*Cap*')",
                    },
                    "ability_filter": {
                        "type": "string",
                        "description": "Optional: filter by ability ('key', 'store', 'copy', 'drop')",
                    },
                },
                "required": ["pattern"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "search_functions",
            "description": "Search for functions matching a pattern across all loaded modules.",
            "parameters": {
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Pattern with * wildcard (e.g., '*new*', '*create*')",
                    },
                    "entry_only": {
                        "type": "boolean",
                        "description": "If true, only return entry functions",
                    },
                },
                "required": ["pattern"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "get_system_object_info",
            "description": "Get information about well-known Sui system objects (Clock, Random, etc.).",
            "parameters": {
                "type": "object",
                "properties": {
                    "object_name": {
                        "type": "string",
                        "enum": ["clock", "random", "deny_list", "system_state"],
                        "description": "Name of the system object",
                    },
                },
                "required": ["object_name"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "create_object",
            "description": "Create/synthesize an object with specific field values in the sandbox.",
            "parameters": {
                "type": "object",
                "properties": {
                    "type_path": {
                        "type": "string",
                        "description": "Full type path (e.g., '0x2::coin::Coin<0x2::sui::SUI>')",
                    },
                    "fields": {
                        "type": "object",
                        "description": "Field values as JSON. Use 'auto' for UID fields.",
                    },
                    "is_shared": {
                        "type": "boolean",
                        "description": "Whether the object is shared (default: false)",
                    },
                },
                "required": ["type_path", "fields"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "execute_ptb",
            "description": "Execute a Programmable Transaction Block in the sandbox. Returns created objects and effects.",
            "parameters": {
                "type": "object",
                "properties": {
                    "calls": {
                        "type": "array",
                        "description": "Array of function calls to execute",
                        "items": {
                            "type": "object",
                            "properties": {
                                "target": {
                                    "type": "string",
                                    "description": "Function target like '0x2::coin::split'",
                                },
                                "type_args": {
                                    "type": "array",
                                    "items": {"type": "string"},
                                    "description": "Type arguments",
                                },
                                "args": {
                                    "type": "array",
                                    "description": "Arguments - objects by ID or pure values",
                                },
                            },
                            "required": ["target"],
                        },
                    },
                },
                "required": ["calls"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "list_objects",
            "description": "List all objects currently in the sandbox with their types and IDs.",
            "parameters": {"type": "object", "properties": {}, "required": []},
        },
    },
    {
        "type": "function",
        "function": {
            "name": "inspect_object",
            "description": "Get detailed information about a specific object including its field values.",
            "parameters": {
                "type": "object",
                "properties": {
                    "object_id": {"type": "string", "description": "Object ID (hex string)"},
                },
                "required": ["object_id"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "get_clock",
            "description": "Get the current sandbox Clock timestamp.",
            "parameters": {"type": "object", "properties": {}, "required": []},
        },
    },
    {
        "type": "function",
        "function": {
            "name": "set_clock",
            "description": "Set the sandbox Clock to a specific timestamp.",
            "parameters": {
                "type": "object",
                "properties": {
                    "timestamp_ms": {
                        "type": "integer",
                        "description": "Timestamp in milliseconds since Unix epoch",
                    },
                },
                "required": ["timestamp_ms"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "validate_type",
            "description": "Validate and parse a Move type string, returning structured type information.",
            "parameters": {
                "type": "object",
                "properties": {
                    "type_str": {
                        "type": "string",
                        "description": "Type string like 'u64', 'vector<u8>', '0x2::coin::Coin<T>'",
                    },
                },
                "required": ["type_str"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "encode_bcs",
            "description": "Encode a value to BCS bytes for a given type.",
            "parameters": {
                "type": "object",
                "properties": {
                    "type_str": {"type": "string", "description": "The type to encode as"},
                    "value": {"description": "The value to encode"},
                },
                "required": ["type_str", "value"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "decode_bcs",
            "description": "Decode BCS bytes to a value for a given type.",
            "parameters": {
                "type": "object",
                "properties": {
                    "type_str": {"type": "string", "description": "The type to decode as"},
                    "bytes_hex": {"type": "string", "description": "Hex-encoded BCS bytes"},
                },
                "required": ["type_str", "bytes_hex"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "disassemble_function",
            "description": "Get Move bytecode disassembly for a function. Useful for understanding what a function does.",
            "parameters": {
                "type": "object",
                "properties": {
                    "module_path": {"type": "string", "description": "Module path"},
                    "function_name": {"type": "string", "description": "Function name"},
                },
                "required": ["module_path", "function_name"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "submit_ptb_plan",
            "description": "Submit the final PTB plan that will create the target types. Call this when you have determined the correct sequence of calls.",
            "parameters": {
                "type": "object",
                "properties": {
                    "calls": {
                        "type": "array",
                        "description": "Final PTB calls to execute",
                        "items": {
                            "type": "object",
                            "properties": {
                                "target": {"type": "string"},
                                "type_args": {"type": "array", "items": {"type": "string"}},
                                "args": {"type": "array"},
                            },
                            "required": ["target"],
                        },
                    },
                    "reasoning": {
                        "type": "string",
                        "description": "Brief explanation of why this PTB will create the target types",
                    },
                },
                "required": ["calls"],
            },
        },
    },
]


@dataclass
class SandboxAgentResult:
    """Result from sandbox agent execution."""

    ptb_plan: dict[str, Any] | None
    tool_calls: list[dict[str, Any]]
    iterations: int
    total_prompt_tokens: int
    total_completion_tokens: int
    final_reasoning: str | None
    success: bool
    error: str | None = None


@dataclass
class SandboxToolExecutor:
    """Executes sandbox tools via Rust binary or in-process."""

    rust_bin: Path
    package_dir: Path
    _process: subprocess.Popen | None = field(default=None, init=False)

    def __post_init__(self):
        # Start the sandbox process
        self._start_sandbox()

    def _start_sandbox(self):
        """Start the sandbox subprocess."""
        cmd = [
            str(self.rust_bin),
            "sandbox-exec",
            "--interactive",
            "--package-dir",
            str(self.package_dir),
        ]
        try:
            self._process = subprocess.Popen(
                cmd,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            logger.info(f"Started sandbox process: {' '.join(cmd)}")
        except Exception as e:
            logger.error(f"Failed to start sandbox: {e}")
            raise

    def execute(self, tool_name: str, args: dict[str, Any]) -> dict[str, Any]:
        """Execute a tool and return the result."""
        # Map tool names to sandbox request format
        request = self._build_request(tool_name, args)

        if self._process is None or self._process.poll() is not None:
            self._start_sandbox()

        try:
            # Send request
            req_json = json.dumps(request) + "\n"
            self._process.stdin.write(req_json)
            self._process.stdin.flush()

            # Read response
            resp_line = self._process.stdout.readline()
            if not resp_line:
                return {"error": "No response from sandbox"}

            return json.loads(resp_line)
        except Exception as e:
            logger.error(f"Tool execution failed: {e}")
            return {"error": str(e)}

    def _build_request(self, tool_name: str, args: dict[str, Any]) -> dict[str, Any]:
        """Build sandbox request from tool call."""
        # Map function names to sandbox actions
        mapping = {
            "list_modules": {"action": "list_modules"},
            "get_struct_info": {
                "action": "inspect_struct",
                "package": args.get("module_path", "").split("::")[0],
                "module": args.get("module_path", "").split("::")[-1] if "::" in args.get("module_path", "") else None,
                "struct_name": args.get("struct_name"),
            },
            "get_function_info": {
                "action": "get_function_info",
                "module_path": args.get("module_path"),
                "function_name": args.get("function_name"),
            },
            "list_functions": {
                "action": "list_functions",
                "module_path": args.get("module_path"),
            },
            "list_structs": {
                "action": "list_structs",
                "module_path": args.get("module_path"),
            },
            "find_constructors": {
                "action": "find_constructors",
                "type_path": args.get("type_path"),
            },
            "search_types": {
                "action": "search_types",
                "pattern": args.get("pattern"),
                "ability_filter": args.get("ability_filter"),
            },
            "search_functions": {
                "action": "search_functions",
                "pattern": args.get("pattern"),
                "entry_only": args.get("entry_only", False),
            },
            "get_system_object_info": {
                "action": "get_system_object_info",
                "object_name": args.get("object_name"),
            },
            "create_object": {
                "action": "create_object",
                "object_type": args.get("type_path"),
                "fields": args.get("fields", {}),
                "is_shared": args.get("is_shared", False),
            },
            "execute_ptb": {
                "action": "execute_ptb",
                "calls": args.get("calls", []),
            },
            "list_objects": {"action": "list_objects"},
            "inspect_object": {
                "action": "inspect_object",
                "object_id": args.get("object_id"),
            },
            "get_clock": {"action": "get_clock"},
            "set_clock": {
                "action": "set_clock",
                "timestamp_ms": args.get("timestamp_ms"),
            },
            "validate_type": {
                "action": "validate_type",
                "type_str": args.get("type_str"),
            },
            "encode_bcs": {
                "action": "encode_bcs",
                "type_str": args.get("type_str"),
                "value": args.get("value"),
            },
            "decode_bcs": {
                "action": "decode_bcs",
                "type_str": args.get("type_str"),
                "bytes_hex": args.get("bytes_hex"),
            },
            "disassemble_function": {
                "action": "disassemble_function",
                "module_path": args.get("module_path"),
                "function_name": args.get("function_name"),
            },
        }

        if tool_name in mapping:
            return mapping[tool_name]

        return {"action": tool_name, **args}

    def close(self):
        """Close the sandbox process."""
        if self._process:
            self._process.terminate()
            self._process.wait(timeout=5)
            self._process = None


class SandboxAgent:
    """
    Agentic LLM agent with full sandbox tool access.

    Uses OpenAI function calling to let the LLM interactively explore
    packages, create objects, and construct PTBs.
    """

    def __init__(
        self,
        cfg: RealAgentConfig,
        rust_bin: Path,
        package_dir: Path,
        client: httpx.Client | None = None,
    ):
        self.cfg = cfg
        self.rust_bin = rust_bin
        self.package_dir = package_dir
        self._client = client or httpx.Client(timeout=120)
        self.is_openrouter = "openrouter.ai" in cfg.base_url.lower()

    def _openrouter_headers(self) -> dict[str, str]:
        return {
            "HTTP-Referer": "https://github.com/MystenLabs/sui-move-interface-extractor",
            "X-Title": "Sui Move Sandbox Agent",
        }

    def run(
        self,
        target_types: list[str],
        package_info: dict[str, Any],
        *,
        timeout_s: float | None = None,
        logger: JsonlLogger | None = None,
        log_context: dict[str, Any] | None = None,
    ) -> SandboxAgentResult:
        """
        Run the agent to construct a PTB that creates the target types.

        Args:
            target_types: List of type paths that should be created
            package_info: Information about the package (modules, structs, functions)
            timeout_s: Maximum time for the entire agent run
            logger: Optional JSONL logger
            log_context: Additional context for logging

        Returns:
            SandboxAgentResult with the final PTB plan and execution trace
        """
        deadline = (time.monotonic() + timeout_s) if timeout_s else None

        # Initialize tool executor
        executor = SandboxToolExecutor(rust_bin=self.rust_bin, package_dir=self.package_dir)

        try:
            return self._run_agent_loop(
                executor=executor,
                target_types=target_types,
                package_info=package_info,
                deadline=deadline,
                logger=logger,
                log_context=log_context or {},
            )
        finally:
            executor.close()

    def _run_agent_loop(
        self,
        executor: SandboxToolExecutor,
        target_types: list[str],
        package_info: dict[str, Any],
        deadline: float | None,
        logger: JsonlLogger | None,
        log_context: dict[str, Any],
    ) -> SandboxAgentResult:
        """Main agent loop with tool calling."""

        # Build initial system prompt
        system_prompt = self._build_system_prompt(target_types, package_info)

        messages = [
            {"role": "system", "content": system_prompt},
            {
                "role": "user",
                "content": f"Create a PTB that constructs instances of these target types: {target_types}\n\n"
                f"Use the available tools to explore the package, understand the constructors, "
                f"and build a working PTB. When ready, call submit_ptb_plan with your final answer.",
            },
        ]

        tool_calls_log = []
        total_prompt_tokens = 0
        total_completion_tokens = 0

        for iteration in range(MAX_ITERATIONS):
            if deadline and time.monotonic() > deadline:
                return SandboxAgentResult(
                    ptb_plan=None,
                    tool_calls=tool_calls_log,
                    iterations=iteration,
                    total_prompt_tokens=total_prompt_tokens,
                    total_completion_tokens=total_completion_tokens,
                    final_reasoning=None,
                    success=False,
                    error="Timeout exceeded",
                )

            # Call LLM with tools
            remaining = (deadline - time.monotonic()) if deadline else None
            response = self._call_llm_with_tools(
                messages=messages,
                tools=SANDBOX_TOOLS,
                timeout_s=remaining,
                logger=logger,
                log_context={**log_context, "iteration": iteration},
            )

            total_prompt_tokens += response["usage"].prompt_tokens
            total_completion_tokens += response["usage"].completion_tokens

            assistant_message = response["message"]
            messages.append(assistant_message)

            # Check for tool calls
            tool_calls = assistant_message.get("tool_calls", [])

            if not tool_calls:
                # No tool calls - check if there's content (final answer without submit_ptb_plan)
                content = assistant_message.get("content", "")
                if content:
                    # Try to extract PTB from content
                    try:
                        ptb = json.loads(content) if "{" in content else None
                        if ptb and "calls" in ptb:
                            return SandboxAgentResult(
                                ptb_plan=ptb,
                                tool_calls=tool_calls_log,
                                iterations=iteration + 1,
                                total_prompt_tokens=total_prompt_tokens,
                                total_completion_tokens=total_completion_tokens,
                                final_reasoning=content,
                                success=True,
                            )
                    except json.JSONDecodeError:
                        pass

                return SandboxAgentResult(
                    ptb_plan=None,
                    tool_calls=tool_calls_log,
                    iterations=iteration + 1,
                    total_prompt_tokens=total_prompt_tokens,
                    total_completion_tokens=total_completion_tokens,
                    final_reasoning=content,
                    success=False,
                    error="No PTB plan submitted",
                )

            # Process tool calls
            tool_results = []
            for tc in tool_calls:
                fn_name = tc["function"]["name"]
                fn_args = json.loads(tc["function"]["arguments"])

                tool_calls_log.append({"name": fn_name, "args": fn_args, "iteration": iteration})

                if logger:
                    logger.event(
                        "tool_call",
                        tool=fn_name,
                        args=fn_args,
                        iteration=iteration,
                        **log_context,
                    )

                # Check for final submission
                if fn_name == "submit_ptb_plan":
                    return SandboxAgentResult(
                        ptb_plan={"calls": fn_args.get("calls", [])},
                        tool_calls=tool_calls_log,
                        iterations=iteration + 1,
                        total_prompt_tokens=total_prompt_tokens,
                        total_completion_tokens=total_completion_tokens,
                        final_reasoning=fn_args.get("reasoning"),
                        success=True,
                    )

                # Execute tool
                result = executor.execute(fn_name, fn_args)

                if logger:
                    logger.event(
                        "tool_result",
                        tool=fn_name,
                        result=result,
                        iteration=iteration,
                        **log_context,
                    )

                tool_results.append(
                    {
                        "role": "tool",
                        "tool_call_id": tc["id"],
                        "content": json.dumps(result),
                    }
                )

            messages.extend(tool_results)

        return SandboxAgentResult(
            ptb_plan=None,
            tool_calls=tool_calls_log,
            iterations=MAX_ITERATIONS,
            total_prompt_tokens=total_prompt_tokens,
            total_completion_tokens=total_completion_tokens,
            final_reasoning=None,
            success=False,
            error=f"Max iterations ({MAX_ITERATIONS}) exceeded",
        )

    def _build_system_prompt(self, target_types: list[str], package_info: dict[str, Any]) -> str:
        """Build the system prompt for the agent."""
        return f"""You are an expert Sui Move developer tasked with creating a Programmable Transaction Block (PTB)
that will construct instances of specific target types.

## Target Types to Create
{json.dumps(target_types, indent=2)}

## Package Information
{json.dumps(package_info, indent=2)}

## Your Task
1. Use the available tools to explore the package and understand its structure
2. Find constructors for the target types (functions that return them)
3. Understand what arguments those constructors need
4. Build a PTB that creates the target types

## Available Tools
You have access to sandbox tools that let you:
- Explore modules, structs, and functions
- Search for constructors and types
- Create objects and execute PTBs in a sandbox
- Inspect results and iterate

## Strategy Tips
- Start by searching for constructors: use find_constructors or search_functions with patterns like "*new*", "*create*"
- Check if constructors need special objects (TreasuryCap, AdminCap, etc.)
- Some types may only be creatable by privileged functions - identify these early
- If a type requires another type, find constructors for dependencies first
- Use execute_ptb to test your plan before final submission

## Final Submission
When you have determined the correct PTB, call submit_ptb_plan with:
- calls: Array of function calls in execution order
- reasoning: Brief explanation of your approach

Remember: The goal is to create instances of ALL target types. If some are impossible to create
(e.g., require admin privileges), explain why in your reasoning."""

    def _call_llm_with_tools(
        self,
        messages: list[dict],
        tools: list[dict],
        timeout_s: float | None,
        logger: JsonlLogger | None,
        log_context: dict[str, Any],
    ) -> dict[str, Any]:
        """Call the LLM with tool definitions."""
        url = f"{self.cfg.base_url}/chat/completions"
        headers = {"Authorization": f"Bearer {self.cfg.api_key}"}
        if self.is_openrouter:
            headers.update(self._openrouter_headers())

        payload = {
            "model": self.cfg.model,
            "temperature": self.cfg.temperature,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
        }

        if self.cfg.max_tokens:
            payload["max_tokens"] = self.cfg.max_tokens

        req_timeout = min(timeout_s, 120) if timeout_s else 120

        try:
            r = self._client.post(url, headers=headers, json=payload, timeout=req_timeout)
            r.raise_for_status()
            data = r.json()
        except Exception as e:
            logger.error(f"LLM request failed: {e}")
            raise

        usage = LLMUsage.from_api_response(data.get("usage"))
        message = data["choices"][0]["message"]

        if logger:
            logger.event(
                "llm_response",
                model=self.cfg.model,
                has_tool_calls=bool(message.get("tool_calls")),
                prompt_tokens=usage.prompt_tokens,
                completion_tokens=usage.completion_tokens,
                **log_context,
            )

        return {"message": message, "usage": usage}


def create_sandbox_agent(
    env_overrides: dict[str, str] | None = None,
    rust_bin: Path | None = None,
    package_dir: Path | None = None,
) -> SandboxAgent:
    """Factory function to create a SandboxAgent."""
    cfg = load_real_agent_config(env_overrides)

    if rust_bin is None:
        # Try to find the rust binary
        rust_bin = Path(__file__).parents[4] / "target" / "release" / "sui_move_interface_extractor"
        if not rust_bin.exists():
            rust_bin = Path(__file__).parents[4] / "target" / "debug" / "sui_move_interface_extractor"

    if package_dir is None:
        package_dir = Path.cwd()

    return SandboxAgent(cfg=cfg, rust_bin=rust_bin, package_dir=package_dir)
