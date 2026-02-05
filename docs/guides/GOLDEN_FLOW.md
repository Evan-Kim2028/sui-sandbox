# Golden Flow: CLI End-to-End

This is the recommended, repeatable CLI workflow for local development.
It uses a dedicated state file so the session is deterministic and easy to debug.

## 1) Create an isolated session

```bash
export SUI_SANDBOX_HOME=/tmp/sui-sandbox-demo
export STATE_FILE="$SUI_SANDBOX_HOME/state.json"
mkdir -p "$SUI_SANDBOX_HOME"
```

## 2) Publish a package (optional but recommended)

```bash
./target/debug/sui-sandbox publish ./examples/convertible_debt --state-file "$STATE_FILE" --json > publish.json
```

If you want the published package ID:

```bash
python3 - <<'PY'
import json
print(json.load(open("publish.json"))["package_address"])
PY
```

## 3) Inspect module interfaces

```bash
./target/debug/sui-sandbox view module 0x2::coin --state-file "$STATE_FILE"
```

## 4) Create a PTB spec

This spec creates a zero SUI coin and then reads its value.

```bash
cat > ptb.json <<'JSON'
{
  "inputs": [],
  "calls": [
    {
      "target": "0x2::coin::zero",
      "type_args": ["0x2::sui::SUI"],
      "args": []
    },
    {
      "target": "0x2::coin::value",
      "type_args": ["0x2::sui::SUI"],
      "args": [{"result": 0}]
    }
  ]
}
JSON
```

## 5) Execute the PTB

```bash
./target/debug/sui-sandbox ptb --spec ptb.json --sender 0x1 --state-file "$STATE_FILE" --json
```

## 6) Generate a real `sui client` command (bridge)

```bash
./target/debug/sui-sandbox bridge ptb --spec ptb.json --state-file "$STATE_FILE"
```

## 7) Check session status

```bash
./target/debug/sui-sandbox status --state-file "$STATE_FILE"
```

## 8) Replay + Analyze loop (debugging)

```bash
# Replay a historical transaction and view PTB-style effects
./target/debug/sui-sandbox replay <DIGEST> --compare

# If replay fails, analyze readiness + missing inputs/packages
./target/debug/sui-sandbox analyze replay <DIGEST>
```

## Notes

- Always pass `--state-file` for reproducibility. Without it, each run starts fresh.
- Package IDs are session-specific in the sandbox. Re-publishing in a new session yields a new ID.
- If a PTB fails, the CLI error will tell you which command index caused it. Use that index to inspect the relevant call in `ptb.json`.
