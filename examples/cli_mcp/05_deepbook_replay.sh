#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/_common.sh"

init_context
replay_pair "D9sMA7x9b8xD6vNJgmhc7N5ja19wAXo45drhsrV1JDva" "deepbook_replay"
