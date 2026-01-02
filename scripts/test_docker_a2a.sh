#!/bin/bash
set -e

REPO_ROOT="$(git rev-parse --show-toplevel)"
CORPUS_HOST="$REPO_ROOT/benchmark/.docker_test_corpus"
RESULTS_HOST="$REPO_ROOT/benchmark/.docker_test_results"
LOGS_HOST="$REPO_ROOT/benchmark/.docker_test_logs"
MANIFEST_HOST="$CORPUS_HOST/manifest.txt"

# Ensure corpus exists
if [ ! -d "$CORPUS_HOST" ]; then
    echo "Corpus not found. Running prepare_test_corpus.sh..."
    "$REPO_ROOT/scripts/prepare_test_corpus.sh"
fi

# Create manifest file
# The package ID must match what's in metadata.json
echo "0x0000000000000000000000000000000000000000000000000000000000000001" > "$MANIFEST_HOST"

echo "Cleaning up old results and logs..."
rm -rf "$RESULTS_HOST"/*
rm -rf "$LOGS_HOST"/*
mkdir -p "$RESULTS_HOST"
mkdir -p "$LOGS_HOST"

echo "Building Docker image smi-bench:test..."
docker build -t smi-bench:test .

# Allow overriding model and agent via env
AGENT="${SMI_AGENT:-real-openai-compatible}"
MODEL="${SMI_MODEL:-google/gemini-2.0-pro-exp-02-05:free}"

echo "Starting container..."
# Mount corpus to /app/corpus
# Mount results to /app/results
# Load .env if it exists
ENV_ARGS=""
if [ -f "$REPO_ROOT/benchmark/.env" ]; then
    echo "Using env-file: benchmark/.env"
    ENV_ARGS="--env-file $REPO_ROOT/benchmark/.env"
fi

CONTAINER_ID=$(docker run -d --rm \
    $ENV_ARGS \
    -e SMI_MODEL="$MODEL" \
    -v "$CORPUS_HOST:/app/corpus" \
    -v "$RESULTS_HOST:/app/results" \
    -v "$LOGS_HOST:/app/logs" \
    -p 9999:9999 \
    smi-bench:test)

echo "Container started: $CONTAINER_ID"

function cleanup {
    echo "Stopping container..."
    docker stop "$CONTAINER_ID"
}
trap cleanup EXIT

echo "Waiting for agent to be ready..."
# Simple retry loop
for i in {1..30}; do
    if curl -s http://localhost:9999/ > /dev/null; then
        echo "Agent is up!"
        break
    fi
    sleep 1
done

# Prepare task payload
# We use the 'message/send' method which is standard for A2A agents in this project.
# The config is passed as a JSON string inside the message text part.
PAYLOAD=$(cat <<EOF
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "message/send",
  "params": {
    "message": {
      "messageId": "docker_smoke_$(date +%s)",
      "role": "user",
      "parts": [
        {
          "text": "{\\"config\\": {\\"corpus_root\\": \\"/app/corpus\\", \\"package_ids_file\\": \\"/app/corpus/manifest.txt\\", \\"agent\\": \\"$AGENT\\", \\"samples\\": 1, \\"simulation_mode\\": \\"dry-run\\", \\"run_id\\": \\"docker_smoke_test\\", \\"continue_on_error\\": true, \\"resume\\": false}, \\"out_dir\\": \\"/app/results\\"}"
        }
      ]
    }
  }
}
EOF
)

echo "Submitting task (Agent: $AGENT, Model: $MODEL)..."
curl -X POST http://localhost:9999/ \
    -H "Content-Type: application/json" \
    -d "$PAYLOAD"

echo ""
echo "Task submitted. Waiting for results (up to 120s)..."

# Poll for result file
for i in {1..120}; do
    if [ -f "$RESULTS_HOST/docker_smoke_test.json" ]; then
        echo "SUCCESS: Result file generated."
        cat "$RESULTS_HOST/docker_smoke_test.json" | head -n 50
        exit 0
    fi
    sleep 1
done

echo "FAILURE: Result file not found after 120s."
docker logs "$CONTAINER_ID"
exit 1
