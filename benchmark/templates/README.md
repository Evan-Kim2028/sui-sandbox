# Prompt Templates

This directory contains prompt templates for the type inhabitation benchmark. Researchers can customize these prompts to experiment with different LLM guidance strategies.

## Files

### `type_inhabitation.txt` (Default)
The main prompt template for the type inhabitation task. This minimal template is sent to the LLM before any retry attempts.

### `type_inhabitation_detailed.txt` (Alternative)
An enhanced prompt template with detailed Move 2024 syntax examples and more explicit guidance. Use this for LLMs that struggle with the minimal template.

### `repair_build_error.txt`
Appended to the prompt when the LLM's generated code fails to compile. Includes build errors for the LLM to fix.

## Template Variables

All templates support these variables (replaced at runtime):

| Variable | Description | Default |
|----------|-------------|---------|
| `{{PACKAGE_ID}}` | The on-chain Sui package ID being tested | - |
| `{{INTERFACE_SUMMARY}}` | Formatted summary of target package's public interface | - |
| `{{MAX_ATTEMPTS}}` | Number of retry attempts allowed | 3 |
| `{{MOVE_EDITION}}` | Move edition to use | 2024.beta |
| `{{BUILD_ERRORS}}` | Build error output (repair template only) | - |
| `{{ATTEMPT_NUMBER}}` | Current attempt number (repair template only) | - |

## Usage

### CLI Flag
```bash
python scripts/e2e_one_package.py --prompt-file templates/type_inhabitation_detailed.txt ...
```

### Default Behavior
If no `--prompt-file` is specified, the built-in prompt (equivalent to `type_inhabitation.txt`) is used.

## Expected LLM Output

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

## Creating Custom Templates

1. Copy an existing template as a starting point
2. Modify the prompt text while keeping template variables
3. Test with `--prompt-file` flag

Tips for effective prompts:
- Be explicit about constraints (LLMs often miss subtle requirements)
- Provide examples of valid output
- Explain evaluation criteria (what counts as success?)
- Include Move-specific guidance (entry function rules, ability requirements)
