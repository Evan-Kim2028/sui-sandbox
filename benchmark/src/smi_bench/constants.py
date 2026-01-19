"""
Centralized constants for smi-bench configuration.

This module provides single-source-of-truth defaults for configuration values
that are used across multiple modules. Using these constants instead of
hardcoded values ensures consistency and makes configuration changes easier.

Environment variable overrides:
- SMI_DEFAULT_RPC_URL: Override the default Sui RPC endpoint
- SMI_SENDER: Default sender address for transactions
"""

from __future__ import annotations

import os

# Default Sui RPC endpoint
# This is the public mainnet fullnode. For production workloads, consider:
# 1. Using a dedicated RPC provider (e.g., Triton, Shinami)
# 2. Running your own fullnode
# 3. Setting SMI_DEFAULT_RPC_URL environment variable
DEFAULT_RPC_URL = os.environ.get(
    "SMI_DEFAULT_RPC_URL",
    "https://fullnode.mainnet.sui.io:443",
)

# Mock sender address used when no real sender is provided
# This address is used for build-only mode and testing
MOCK_SENDER_ADDRESS = "0x0"

# Default agent for benchmarking
DEFAULT_AGENT = "real-openai-compatible"

# Default simulation mode
DEFAULT_SIMULATION_MODE = "dry-run"

# Default timeout for per-package processing (seconds)
DEFAULT_PER_PACKAGE_TIMEOUT_SECONDS = 300.0

# Default maximum plan attempts per package
DEFAULT_MAX_PLAN_ATTEMPTS = 2

# RPC request timeout (seconds)
RPC_REQUEST_TIMEOUT_SECONDS = 30.0

# Health check timeout (seconds)
HEALTH_CHECK_TIMEOUT_SECONDS = 5.0

# =============================================================================
# Retry Configuration
# =============================================================================

# Default retry settings for RPC and subprocess operations
DEFAULT_RETRY_MAX_ATTEMPTS = 3
DEFAULT_RETRY_BASE_DELAY = 2.0  # seconds
DEFAULT_RETRY_MAX_DELAY = 30.0  # seconds

# Infrastructure retry settings
INFRA_RETRY_MAX_ATTEMPTS = 3
INFRA_RETRY_DELAY_SECONDS = 5.0

# Real agent request retry settings
AGENT_REQUEST_MAX_RETRIES = 6
AGENT_REQUEST_MIN_TIMEOUT = 60.0  # seconds
AGENT_REQUEST_BACKOFF_INITIAL = 1.0  # seconds
AGENT_REQUEST_BACKOFF_MAX = 8.0  # seconds

# =============================================================================
# Gas Budget Defaults
# =============================================================================

# Default gas budget in MIST (1 SUI = 10^9 MIST)
DEFAULT_GAS_BUDGET = 10_000_000

# Gas budget ladder for retries (comma-separated string of increasing budgets)
DEFAULT_GAS_BUDGET_LADDER = "20000000,50000000"

# =============================================================================
# Framework Addresses
# =============================================================================

# Sui framework package addresses (both short and full forms for matching)
FRAMEWORK_ADDRESSES = frozenset(
    {
        "0x1",
        "0x2",
        "0x3",
        "0x0000000000000000000000000000000000000000000000000000000000000001",
        "0x0000000000000000000000000000000000000000000000000000000000000002",
        "0x0000000000000000000000000000000000000000000000000000000000000003",
    }
)

# Helper package address (zero address)
HELPER_PACKAGE_ADDRESS = "0x0000000000000000000000000000000000000000000000000000000000000000"
