#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/_common.sh"

init_context
replay_pair "FbrMKMyzWm1K89qBZ45sYfCDsEtNmcnBdU9xiT7NKvmR" "deepbook_cancel_1"
replay_pair "7aQBpHjvgNguGB4WoS9h8ZPgrAPfDqae25BZn5MxXoWY" "deepbook_cancel_2"
replay_pair "3AKpMt66kXcPutKxkQ4D3NuAu4MJ1YGEvTNkWoAzyVVE" "deepbook_limit_1"
replay_pair "6fZMHYnpJoShz6ZXuWW14dCTgwv9XpgZ4jbZh6HBHufU" "deepbook_limit_2"
