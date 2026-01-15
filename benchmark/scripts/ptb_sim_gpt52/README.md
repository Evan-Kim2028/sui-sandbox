# PTB Simulation Tests (GPT-5.2)

Tests LLM's ability to simulate real mainnet PTBs from scratch.

## Overview

We give the LLM randomly picked PTBs from Sui mainnet. The LLM has **only 3 iterations** to:

1. **Iteration 1**: Explore loaded modules via introspection tools
2. **Iteration 2**: Write and compile any needed Move modules
3. **Iteration 3**: Execute the PTB and submit solution

The LLM must work efficiently, using batched tool calls to maximize what it learns per iteration.

## Scripts

- `test_gpt52_multi.py` - Tests multiple DeFi PTBs (DeepBook orders, flashloan swaps)
- `test_gpt52_clmm_swap.py` - Tests CLMM Multi-Swap transaction (21 commands)

## Batched Tool Calls

The LLM can call multiple sandbox tools per iteration for efficiency:

```json
[
  {"tool": "list_modules", "args": {}},
  {"tool": "search_functions", "args": {"pattern": "*swap*"}},
  {"tool": "get_function_info", "args": {"module_path": "0x2::coin", "function_name": "zero"}}
]
```

## Usage

```bash
OPENROUTER_API_KEY=... python benchmark/scripts/ptb_sim_gpt52/test_gpt52_multi.py
```

Or with a specific model:

```bash
SMI_MODEL=openai/gpt-5.2 python benchmark/scripts/ptb_sim_gpt52/test_gpt52_clmm_swap.py
```

## Available Sandbox Tools

### Introspection
- `list_modules` - List all loaded Move modules
- `list_functions` - List functions in a module
- `list_structs` - List structs in a module
- `get_function_info` - Get function signature details
- `get_struct_info` - Get struct type definition
- `find_constructors` - Find functions that return a given type
- `search_functions` - Pattern search across all modules
- `disassemble_function` - Get bytecode disassembly

### Compilation
- `compile_move` - Compile Move source and deploy to sandbox

### Execution
- `execute_ptb` - Execute a programmable transaction block
- `submit_solution` - Submit final solution
