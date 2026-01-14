#!/usr/bin/env python3
"""
Production LLM Sandbox Integration

This script provides REAL integration between the LLM and the Rust sandbox:
1. Real Sandbox Integration - Calls the Rust binary for actual execution
2. Move Compilation - Actually compiles Move modules via `sui move build`
3. Object State Inspection - Extracts struct definitions from bytecode
4. Comprehensive Logging - Saves all LLM reasoning for troubleshooting
5. Expanded Error Codes - Maps abort codes to actionable suggestions

Usage:
    python3 llm_sandbox_production.py --target-function "pool::swap"
    python3 llm_sandbox_production.py --target-package 0x91bf...
"""

import argparse
import asyncio
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import httpx

# =============================================================================
# Configuration
# =============================================================================

ENV_PATH = Path(__file__).parent / ".env"
if ENV_PATH.exists():
    for line in ENV_PATH.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            os.environ.setdefault(key, value)

OPENROUTER_API_KEY = os.environ.get("OPENROUTER_API_KEY", "")
OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1"
PROJECT_ROOT = Path(__file__).parent.parent
RUST_BINARY = PROJECT_ROOT / "target/release/sui_move_interface_extractor"
TX_CACHE_DIR = PROJECT_ROOT / ".tx-cache"
LOG_DIR = PROJECT_ROOT / "benchmark" / "llm_logs"

# GPT-5.2 model
DEFAULT_MODEL = "openai/gpt-5.2"

# =============================================================================
# Abort Code Reference (expanded from errors.rs and simulation.rs)
# =============================================================================

ABORT_CODE_REFERENCE = {
    # Common framework abort codes
    0: {
        "name": "E_GENERIC_ABORT",
        "meaning": "Generic assertion failed (assert!(condition))",
        "likely_cause": "A boolean condition was false",
        "fix_hint": "Check the function's preconditions - often related to object state"
    },
    1: {
        "name": "E_NOT_AUTHORIZED / E_NOT_OWNER",
        "meaning": "Permission/authorization check failed",
        "likely_cause": "Sender doesn't own the object or lack required capability",
        "fix_hint": "Use env.set_sender() to match the object owner, or create a capability object"
    },
    2: {
        "name": "E_INVALID_ARGUMENT / E_PERMISSION_DENIED",
        "meaning": "Invalid argument or permission denied",
        "likely_cause": "Argument doesn't meet requirements or sender lacks permission",
        "fix_hint": "Check argument constraints, ensure sender has required role"
    },
    3: {
        "name": "E_INVALID_STATE",
        "meaning": "Object is in invalid state for operation",
        "likely_cause": "Object state machine requires different state",
        "fix_hint": "Initialize object to correct state before calling"
    },
    7: {
        "name": "E_INSUFFICIENT_BALANCE",
        "meaning": "Not enough balance for operation",
        "likely_cause": "Coin balance too low for transfer/swap",
        "fix_hint": "Create coin with larger balance using env.create_coin()"
    },
    202: {
        "name": "E_VERSION_MISMATCH",
        "meaning": "Protocol/object version mismatch",
        "likely_cause": "Object has wrong version field value",
        "fix_hint": "Set object's version field to expected value (usually 1)"
    },
    303: {
        "name": "E_INVALID_POOL_STATE",
        "meaning": "Pool state invalid for operation",
        "likely_cause": "Pool liquidity=0 or sqrt_price invalid",
        "fix_hint": "Initialize pool with liquidity > 0 and valid sqrt_price"
    },
    1000: {
        "name": "E_NOT_SUPPORTED",
        "meaning": "Native function not supported in sandbox",
        "likely_cause": "Called a native that isn't mocked",
        "fix_hint": "This operation cannot be tested in sandbox"
    },
    65537: {  # 0x10001
        "name": "E_NOT_SYSTEM_ADDRESS",
        "meaning": "Operation requires system address (0x0)",
        "likely_cause": "Privileged operation called from non-system sender",
        "fix_hint": "This is a system-only operation, cannot be tested normally"
    },
}

# =============================================================================
# Logging System
# =============================================================================

class ReasoningLogger:
    """Logs all LLM reasoning and sandbox interactions for troubleshooting."""

    def __init__(self, session_id: str):
        self.session_id = session_id
        self.log_dir = LOG_DIR / session_id
        self.log_dir.mkdir(parents=True, exist_ok=True)
        self.log_file = self.log_dir / "reasoning.jsonl"
        self.summary_file = self.log_dir / "summary.json"
        self.events: List[Dict] = []
        self.start_time = time.time()

    def log(self, event_type: str, data: Dict):
        """Log an event with timestamp."""
        event = {
            "timestamp": datetime.now().isoformat(),
            "elapsed_ms": int((time.time() - self.start_time) * 1000),
            "type": event_type,
            **data
        }
        self.events.append(event)

        # Append to JSONL file immediately
        with open(self.log_file, "a") as f:
            f.write(json.dumps(event) + "\n")

    def log_llm_request(self, messages: List[Dict], iteration: int):
        """Log LLM request."""
        self.log("llm_request", {
            "iteration": iteration,
            "message_count": len(messages),
            "last_user_message": messages[-1]["content"][:500] if messages else ""
        })

    def log_llm_response(self, response: str, parsed_action: Optional[Dict], iteration: int):
        """Log LLM response."""
        self.log("llm_response", {
            "iteration": iteration,
            "response_length": len(response),
            "response_preview": response[:1000],
            "parsed_action": parsed_action,
            "parse_success": parsed_action is not None
        })

    def log_sandbox_execution(self, command: str, output: str, success: bool, iteration: int):
        """Log sandbox execution."""
        self.log("sandbox_execution", {
            "iteration": iteration,
            "command": command,
            "success": success,
            "output_length": len(output),
            "output_preview": output[:2000]
        })

    def log_move_compilation(self, project_path: str, success: bool, output: str):
        """Log Move compilation attempt."""
        self.log("move_compilation", {
            "project_path": project_path,
            "success": success,
            "output": output[:2000]
        })

    def log_struct_inspection(self, package_id: str, structs: List[Dict]):
        """Log struct definition extraction."""
        self.log("struct_inspection", {
            "package_id": package_id,
            "struct_count": len(structs),
            "structs": structs
        })

    def save_summary(self, success: bool, iterations: int, final_state: Dict):
        """Save session summary."""
        summary = {
            "session_id": self.session_id,
            "success": success,
            "total_iterations": iterations,
            "total_time_ms": int((time.time() - self.start_time) * 1000),
            "final_state": final_state,
            "event_count": len(self.events)
        }
        with open(self.summary_file, "w") as f:
            json.dump(summary, f, indent=2)
        return summary

# =============================================================================
# Sandbox Integration
# =============================================================================

class RealSandbox:
    """Real integration with the Rust sandbox binary via sandbox-exec CLI."""

    def __init__(self, logger: ReasoningLogger):
        self.binary = RUST_BINARY
        if not self.binary.exists():
            self.binary = PROJECT_ROOT / "target/debug/sui_move_interface_extractor"
        self.logger = logger
        self.temp_projects: List[Path] = []

    def _call_sandbox_exec(self, request: Dict) -> Dict:
        """Call the sandbox-exec CLI with a JSON request."""
        try:
            result = subprocess.run(
                [str(self.binary), "sandbox-exec", "--input", "-", "--output", "-"],
                input=json.dumps(request),
                capture_output=True,
                text=True,
                timeout=120,
                cwd=str(PROJECT_ROOT),
            )

            if result.returncode == 0:
                return json.loads(result.stdout)
            else:
                return {"success": False, "error": result.stderr or result.stdout}
        except subprocess.TimeoutExpired:
            return {"success": False, "error": "Sandbox execution timed out"}
        except json.JSONDecodeError as e:
            return {"success": False, "error": f"Failed to parse response: {e}"}
        except Exception as e:
            return {"success": False, "error": str(e)}

    def extract_struct_definitions(self, package_id: str) -> List[Dict]:
        """Extract struct definitions from a package using sandbox-exec."""
        request = {
            "action": "inspect_struct",
            "package": package_id,
            "module": None,
            "struct_name": None,
        }

        response = self._call_sandbox_exec(request)

        if response.get("success") and response.get("data"):
            structs = response["data"].get("structs", [])
            self.logger.log_struct_inspection(package_id, structs)
            return structs

        self.logger.log("struct_inspection_error", {
            "package_id": package_id,
            "error": response.get("error", "Unknown error")
        })
        return []

    def create_object(self, object_type: str, fields: Dict) -> Tuple[bool, str]:
        """Create an object in the sandbox."""
        request = {
            "action": "create_object",
            "object_type": object_type,
            "fields": fields,
            "object_id": None,
        }

        response = self._call_sandbox_exec(request)

        if response.get("success"):
            object_id = response.get("data", {}).get("object_id", "unknown")
            return True, object_id
        else:
            return False, response.get("error", "Unknown error")

    def execute_ptb(self, inputs: List[Dict], commands: List[Dict]) -> Tuple[bool, str, Dict]:
        """Execute a PTB via sandbox-exec."""
        request = {
            "action": "execute_ptb",
            "inputs": inputs,
            "commands": commands,
        }

        response = self._call_sandbox_exec(request)

        error_info = {
            "has_error": not response.get("success", False),
            "abort_code": response.get("abort_code"),
            "module": None,
            "function": None,
        }

        if response.get("abort_module"):
            parts = response["abort_module"].split("::")
            if len(parts) >= 2:
                error_info["module"] = parts[0]
                error_info["function"] = parts[1]

        output = json.dumps(response, indent=2)
        return response.get("success", False), output, error_info

    def load_module(self, bytecode_path: str) -> Tuple[bool, str]:
        """Load compiled module bytecode into the sandbox."""
        request = {
            "action": "load_module",
            "bytecode_path": bytecode_path,
            "module_name": None,
        }

        response = self._call_sandbox_exec(request)

        if response.get("success"):
            data = response.get("data", {})
            return True, f"Loaded {data.get('modules_loaded', 0)} modules at {data.get('package_address', 'unknown')}"
        else:
            return False, response.get("error", "Unknown error")

    def compile_move_module(self, module_code: str, module_name: str) -> Tuple[bool, str, Optional[Path]]:
        """Actually compile a Move module using sui move build."""
        # Create temp project
        temp_dir = Path(tempfile.mkdtemp(prefix="move_module_"))
        self.temp_projects.append(temp_dir)

        try:
            # Create project structure
            sources_dir = temp_dir / "sources"
            sources_dir.mkdir()

            # Write Move.toml
            move_toml = f"""[package]
name = "{module_name}"
version = "0.0.1"
edition = "2024.beta"

[dependencies]
Sui = {{ git = "https://github.com/MystenLabs/sui.git", subdir = "crates/sui-framework/packages/sui-framework", rev = "framework/mainnet" }}

[addresses]
{module_name} = "0x0"
"""
            (temp_dir / "Move.toml").write_text(move_toml)

            # Write module source
            source_file = sources_dir / f"{module_name}.move"
            source_file.write_text(module_code)

            # Run sui move build
            result = subprocess.run(
                ["sui", "move", "build", "--path", str(temp_dir)],
                capture_output=True,
                text=True,
                timeout=120,
            )

            output = result.stdout + result.stderr
            success = result.returncode == 0

            self.logger.log_move_compilation(str(temp_dir), success, output)

            if success:
                # Find compiled bytecode
                build_dir = temp_dir / "build" / module_name / "bytecode_modules"

                # Load the compiled bytecode into sandbox
                if build_dir.exists():
                    load_success, load_msg = self.load_module(str(build_dir))
                    if load_success:
                        output += f"\n{load_msg}"
                    else:
                        output += f"\nWarning: Failed to load bytecode: {load_msg}"

                return True, output, build_dir
            else:
                return False, output, None

        except subprocess.TimeoutExpired:
            self.logger.log_move_compilation(str(temp_dir), False, "TIMEOUT")
            return False, "Compilation timed out", None
        except FileNotFoundError:
            return False, "sui CLI not found - install with: cargo install --locked --git https://github.com/MystenLabs/sui.git sui", None

    def cleanup(self):
        """Clean up temporary directories."""
        for temp_dir in self.temp_projects:
            try:
                shutil.rmtree(temp_dir)
            except Exception:
                pass

# =============================================================================
# LLM Client
# =============================================================================

class LLMClient:
    """Client for OpenRouter API with GPT-5.2."""

    def __init__(self, api_key: str, model: str = DEFAULT_MODEL):
        self.api_key = api_key
        self.model = model
        self.base_url = OPENROUTER_BASE_URL
        self.client = httpx.AsyncClient(timeout=180.0)

    async def chat(self, messages: List[Dict], temperature: float = 0.0) -> str:
        """Send a chat completion request."""
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://github.com/anthropics/claude-code",
            "X-Title": "Sui Move Sandbox Production",
        }

        payload = {
            "model": self.model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": 8192,
        }

        response = await self.client.post(
            f"{self.base_url}/chat/completions",
            headers=headers,
            json=payload,
        )
        response.raise_for_status()

        data = response.json()
        return data["choices"][0]["message"]["content"]

    async def close(self):
        await self.client.aclose()

# =============================================================================
# System Prompt with Real Capabilities
# =============================================================================

SYSTEM_PROMPT = """You are an expert Sui Move developer working with a REAL local sandbox environment.

## Your Goal
Achieve TYPE INHABITATION for Move functions - call them with valid arguments without aborting.

## REAL Sandbox Capabilities
This is NOT a simulation. Your actions will ACTUALLY:
1. Compile Move code using `sui move build`
2. Deploy bytecode to the local VM
3. Execute PTB commands through the Move VM
4. Return real error messages from actual execution

## Available Actions (JSON format)

### WRITE_MOVE_MODULE - Actually compiles via sui move build
```json
{
  "action": "WRITE_MOVE_MODULE",
  "module_name": "test_helpers",
  "module_code": "module test_helpers::helpers { use sui::object::{Self, UID}; ... }",
  "explanation": "Why this module helps"
}
```
NOTE: Use proper Move syntax. The module WILL be compiled.

### CREATE_OBJECT - Create object in sandbox memory
```json
{
  "action": "CREATE_OBJECT",
  "object_type": "0x2::coin::Coin<0x2::sui::SUI>",
  "fields": {"balance": 1000000000},
  "explanation": "Why this object is needed"
}
```

### EXECUTE_PTB - Execute real PTB through Move VM
```json
{
  "action": "EXECUTE_PTB",
  "commands": [...],
  "explanation": "What this achieves"
}
```

### INSPECT_STRUCT - Get struct definition from bytecode
```json
{
  "action": "INSPECT_STRUCT",
  "package_id": "0x91bf...",
  "explanation": "Why we need to see the struct definition"
}
```

### SUCCESS / GIVE_UP
```json
{"action": "SUCCESS", "summary": "How we achieved type inhabitation"}
{"action": "GIVE_UP", "reason": "Why this cannot be solved"}
```

## Abort Code Reference
- 0: Generic assertion failed - check preconditions
- 1: Not authorized/not owner - fix sender or create capability
- 2: Invalid argument or permission denied
- 7: Insufficient balance - create coin with more balance
- 202: Version mismatch - set object version field correctly
- 1000: Unsupported native function (sandbox limitation)

## Strategy for DeFi Functions
For complex DeFi (pools, swaps), DON'T try to replicate mainnet state. Instead:

1. INSPECT_STRUCT to see what fields Pool needs
2. WRITE_MOVE_MODULE to create helper that builds valid Pool:
   - version = 1 (or expected version)
   - liquidity > 0
   - valid sqrt_price
3. Execute PTB that calls your helper, then the target function

## Important
- Write VALID Move code - it will actually compile
- struct fields must match exactly what the bytecode expects
- Use proper BCS encoding for object bytes

Respond with ONLY valid JSON matching an action format."""

# =============================================================================
# Main Self-Healing Loop
# =============================================================================

@dataclass
class SandboxState:
    """Tracks sandbox state."""
    deployed_modules: List[str] = field(default_factory=list)
    created_objects: List[Dict] = field(default_factory=list)
    known_structs: Dict[str, List[Dict]] = field(default_factory=dict)
    iteration: int = 0


async def run_production_loop(
    target_info: Dict,
    llm: LLMClient,
    sandbox: RealSandbox,
    logger: ReasoningLogger,
    max_iterations: int = 10,
) -> Tuple[bool, Dict]:
    """Run production self-healing loop with real sandbox."""

    state = SandboxState()
    messages = []

    # Build initial context
    initial_context = f"""## Target
Package: {target_info.get('package', 'unknown')}
Module: {target_info.get('module', 'unknown')}
Function: {target_info.get('function', 'unknown')}
Type Arguments: {target_info.get('type_args', [])}
Expected Arguments: {target_info.get('args', [])}

## Initial Sandbox State
- Sui Framework (0x1, 0x2, 0x3) is loaded
- No custom packages deployed yet
- Target package bytecode is available

## First Step
I recommend starting with INSPECT_STRUCT to understand what fields the target types need.

What's your first action?"""

    messages = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": initial_context},
    ]

    for iteration in range(max_iterations):
        state.iteration = iteration + 1
        print(f"\n{'='*60}")
        print(f"ITERATION {state.iteration}/{max_iterations}")
        print(f"{'='*60}")

        # Get LLM response
        print("\n📤 Sending to LLM...")
        logger.log_llm_request(messages, iteration + 1)

        try:
            response = await llm.chat(messages)
        except Exception as e:
            print(f"❌ LLM API error: {e}")
            logger.log("llm_error", {"iteration": iteration + 1, "error": str(e)})
            continue

        print(f"\n📥 Response preview: {response[:300]}...")

        # Parse action
        try:
            if "```json" in response:
                json_str = response.split("```json")[1].split("```")[0]
            elif "```" in response:
                json_str = response.split("```")[1].split("```")[0]
            else:
                json_str = response
            action = json.loads(json_str.strip())
            logger.log_llm_response(response, action, iteration + 1)
        except json.JSONDecodeError as e:
            print(f"⚠️ JSON parse error: {e}")
            logger.log_llm_response(response, None, iteration + 1)
            messages.append({"role": "assistant", "content": response})
            messages.append({"role": "user", "content": f"Invalid JSON: {e}. Respond with ONLY valid JSON."})
            continue

        action_type = action.get("action", "unknown")
        print(f"\n🎯 Action: {action_type}")

        # Handle actions
        if action_type == "SUCCESS":
            print(f"\n✅ SUCCESS: {action.get('summary', 'N/A')}")
            final_state = {
                "deployed_modules": state.deployed_modules,
                "created_objects": state.created_objects,
                "iterations": state.iteration
            }
            logger.save_summary(True, state.iteration, final_state)
            return True, final_state

        elif action_type == "GIVE_UP":
            print(f"\n❌ GIVE_UP: {action.get('reason', 'N/A')}")
            final_state = {"reason": action.get("reason")}
            logger.save_summary(False, state.iteration, final_state)
            return False, final_state

        elif action_type == "INSPECT_STRUCT":
            package_id = action.get("package_id", "")
            print(f"\n🔍 Inspecting structs in {package_id[:20]}...")

            structs = sandbox.extract_struct_definitions(package_id)
            state.known_structs[package_id] = structs

            if structs:
                struct_info = "\n".join([
                    f"  {s['module']}::{s['name']}: fields={s['fields']}, abilities={s['abilities']}"
                    for s in structs[:10]
                ])
                result_msg = f"""Struct definitions for {package_id}:

{struct_info}

{'(showing first 10 of ' + str(len(structs)) + ')' if len(structs) > 10 else ''}

Now you know the exact struct layout. What's your next action?"""
            else:
                result_msg = f"""Could not extract struct definitions for {package_id}.
The package may not be in cache. Try a different approach.

What's your next action?"""

            messages.append({"role": "assistant", "content": response})
            messages.append({"role": "user", "content": result_msg})

        elif action_type == "WRITE_MOVE_MODULE":
            module_name = action.get("module_name", "test_helpers")
            module_code = action.get("module_code", "")
            print(f"\n📝 Compiling Move module: {module_name}")

            success, output, build_path = sandbox.compile_move_module(module_code, module_name)

            if success:
                state.deployed_modules.append(module_name)
                result_msg = f"""✅ Module compiled successfully!

Module: {module_name}
Bytecode location: {build_path}

The module is now available. What's your next action to use it?"""
            else:
                # Parse compile errors for LLM
                result_msg = f"""❌ Compilation failed:

{output[:2000]}

Fix the Move code and try again. Common issues:
- Missing imports (use sui::object, use sui::transfer, etc.)
- Type mismatches
- Missing abilities on structs

What's your corrected module code?"""

            messages.append({"role": "assistant", "content": response})
            messages.append({"role": "user", "content": result_msg})

        elif action_type == "CREATE_OBJECT":
            obj_type = action.get("object_type", "")
            fields = action.get("fields", {})
            print(f"\n📦 Creating object: {obj_type}")

            # Create object via real sandbox
            create_success, object_id = sandbox.create_object(obj_type, fields)

            if create_success:
                state.created_objects.append({"type": obj_type, "fields": fields, "id": object_id})
                result_msg = f"""Object created in sandbox:
- Type: {obj_type}
- Fields: {fields}
- ID: {object_id}

Object is available for PTB execution. What's your next action?"""
            else:
                result_msg = f"""❌ Failed to create object:
- Type: {obj_type}
- Error: {object_id}

Check the object type and fields and try again. What's your next action?"""

            messages.append({"role": "assistant", "content": response})
            messages.append({"role": "user", "content": result_msg})

        elif action_type == "EXECUTE_PTB":
            commands = action.get("commands", [])
            print(f"\n🚀 Executing PTB with {len(commands)} commands...")

            # Convert commands to sandbox format
            ptb_inputs = []
            ptb_commands = []

            for cmd in commands:
                if isinstance(cmd, dict):
                    # Convert LLM command format to sandbox format
                    if cmd.get("type") == "move_call":
                        ptb_commands.append({
                            "type": "move_call",
                            "package": cmd.get("package", ""),
                            "module": cmd.get("module", ""),
                            "function": cmd.get("function", ""),
                            "type_args": cmd.get("type_args", []),
                            "args": cmd.get("args", []),
                        })

            # Execute via real sandbox
            success, output, error_info = sandbox.execute_ptb(ptb_inputs, ptb_commands)

            # Log the execution
            logger.log_sandbox_execution("execute_ptb", output, success, state.iteration)

            if success:
                result_msg = f"""✅ PTB Execution SUCCESS!

{output[-1000:]}

Type inhabitation achieved. Declare SUCCESS with your summary."""
            else:
                # Build detailed error feedback
                error_details = []
                if error_info.get("abort_code") is not None:
                    code = error_info["abort_code"]
                    error_details.append(f"Abort code: {code}")
                    if error_info.get("abort_meaning"):
                        error_details.append(f"Meaning: {error_info['abort_meaning']}")
                    if error_info.get("fix_hint"):
                        error_details.append(f"Fix hint: {error_info['fix_hint']}")

                if error_info.get("module"):
                    error_details.append(f"Location: {error_info['module']}::{error_info.get('function', '?')}")

                result_msg = f"""❌ PTB Execution FAILED

{chr(10).join(error_details) if error_details else 'Unknown error'}

Raw output (last 1500 chars):
{output[-1500:]}

Analyze the error and take corrective action. What's your next step?"""

            messages.append({"role": "assistant", "content": response})
            messages.append({"role": "user", "content": result_msg})

        else:
            print(f"⚠️ Unknown action: {action_type}")
            messages.append({"role": "assistant", "content": response})
            messages.append({"role": "user", "content": f"Unknown action. Use: INSPECT_STRUCT, WRITE_MOVE_MODULE, CREATE_OBJECT, EXECUTE_PTB, SUCCESS, GIVE_UP"})

    print(f"\n⏰ Max iterations reached")
    final_state = {"max_iterations_reached": True}
    logger.save_summary(False, max_iterations, final_state)
    return False, final_state


# =============================================================================
# Main Entry Point
# =============================================================================

async def main():
    parser = argparse.ArgumentParser(description="Production LLM Sandbox Integration")
    parser.add_argument("--model", default=DEFAULT_MODEL, help=f"Model (default: {DEFAULT_MODEL})")
    parser.add_argument("--max-iterations", type=int, default=10, help="Max iterations")
    parser.add_argument("--target-package", default="0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1",
                        help="Target package ID")
    parser.add_argument("--target-module", default="pool", help="Target module")
    parser.add_argument("--target-function", default="swap", help="Target function")
    args = parser.parse_args()

    if not OPENROUTER_API_KEY:
        print("❌ OPENROUTER_API_KEY not set")
        sys.exit(1)

    # Create session
    session_id = datetime.now().strftime("%Y%m%d_%H%M%S")
    logger = ReasoningLogger(session_id)

    print("=" * 60)
    print("🔬 Production LLM Sandbox Integration")
    print("=" * 60)
    print(f"Session: {session_id}")
    print(f"Model: {args.model}")
    print(f"Logs: {logger.log_dir}")

    target_info = {
        "package": args.target_package,
        "module": args.target_module,
        "function": args.target_function,
        "type_args": ["0x2::sui::SUI", "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC"],
        "args": ["Pool<SUI, USDC>", "Coin<SUI>", "u64", "bool"],
    }

    print(f"\n🎯 Target: {target_info['module']}::{target_info['function']}")

    llm = LLMClient(OPENROUTER_API_KEY, args.model)
    sandbox = RealSandbox(logger)

    try:
        success, final_state = await run_production_loop(
            target_info, llm, sandbox, logger, args.max_iterations
        )

        print("\n" + "=" * 60)
        print("📊 FINAL RESULT")
        print("=" * 60)
        print(f"Success: {'✅' if success else '❌'}")
        print(f"Iterations: {final_state.get('iterations', 'N/A')}")
        print(f"Log file: {logger.log_file}")

    finally:
        await llm.close()
        sandbox.cleanup()


if __name__ == "__main__":
    asyncio.run(main())
