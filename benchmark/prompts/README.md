# Prompt Templates

This directory contains prompt templates for the type inhabitation benchmark. Researchers can customize these prompts to experiment with different LLM guidance strategies.

## Files

### `type_inhabitation_v1.txt`
The main prompt for the type inhabitation task. This is the initial prompt sent to the LLM before any retry attempts.

**Template Variables:**
- `{{PACKAGE_ID}}` - The on-chain Sui package ID being tested
- `{{INTERFACE_SUMMARY}}` - A formatted summary of the target package's public types and functions
- `{{MAX_ATTEMPTS}}` - Number of retry attempts allowed (default: 3)
- `{{MOVE_EDITION}}` - Move edition to use (default: 2024.beta)

### `repair_build_error.txt`
Appended to the prompt when the LLM's generated code fails to compile. Includes build errors for the LLM to fix.

**Template Variables:**
- `{{BUILD_ERRORS}}` - The error output from `sui move build`
- `{{ATTEMPT_NUMBER}}` - Current attempt (1-indexed)
- `{{MAX_ATTEMPTS}}` - Total attempts allowed

## Usage

To use a custom prompt, set the environment variable:

```bash
export SMI_PROMPT_DIR=/path/to/your/prompts
```

Or pass the `--prompt-dir` flag to the e2e script (TODO: implement this).

## Schema: helper_pkg_v1

The LLM must return a JSON object with this structure:

```json
{
  "move_toml": "string - full contents of Move.toml",
  "files": {
    "sources/helper.move": "string - Move source code",
    "sources/other.move": "string - optional additional files"
  },
  "entrypoints": [
    {"target": "helper_pkg::helper::my_func"}
  ],
  "assumptions": [
    "string - explanation of approach taken"
  ]
}
```

## Prompt Engineering Tips

1. **Be explicit about constraints** - LLMs often miss subtle requirements
2. **Provide examples** - Show what valid output looks like
3. **Explain the evaluation criteria** - What counts as success?
4. **Include Move-specific guidance** - Entry function rules, ability requirements, etc.

## Experimental Prompts

Create new prompt files (e.g., `type_inhabitation_v2.txt`) to experiment with:
- Chain-of-thought prompting
- Few-shot examples
- More/less prescriptive guidance
- Different output formats
