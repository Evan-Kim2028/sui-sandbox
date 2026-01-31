#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/_common.sh"

init_context
replay_pair "CTzkozkRKga4NvaKLptStVUdRbLbtPqR22FkuK6WVUQ4" "scallop_deposit"
