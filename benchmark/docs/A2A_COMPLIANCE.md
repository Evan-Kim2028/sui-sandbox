# A2A Protocol Compliance & Testing

This document describes the Agent2Agent (A2A) protocol implementation, compliance status, and comprehensive testing strategy for the sui-move-interface-extractor benchmark.

## Overview

The sui-move-interface-extractor benchmark implements Google's [Agent2Agent (A2A) Protocol](https://a2a-protocol.org/) for agent interoperability. This implementation focuses on the benchmark execution lifecycle, providing a standardized interface for benchmark runners and evaluators.

## Protocol Version

**Implemented Version:** `0.3.0`

Compliance is signaled via:
- Agent card `protocol_version` field: `0.3.0`
- HTTP response header: `A2A-Version: 0.3.0`

---

## Compliance Implementation

### ✅ Task Lifecycle Management
The green agent (`smi-a2a-green`) implements full task lifecycle support per A2A spec section 3.1:
- **Task States:** `submitted` → `working` → `completed/failed/canceled`
- **Streaming Updates:** Real-time status updates via Server-Sent Events (SSE).
- **Task Artifacts:** Produces structured artifacts including evaluation bundles and raw results.
- **Task Cancellation:** Supports graceful subprocess termination using a SIGTERM → SIGKILL pattern with a 5-second grace period.

### ✅ Protocol Version Headers
All HTTP responses include the `A2A-Version: 0.3.0` header, implemented via Starlette middleware (`A2AVersionMiddleware`).

### ✅ Agent Card Discovery
Agent cards are available at the standardized `/.well-known/agent-card.json` endpoint for both green and purple agents.

### ✅ Streaming Support
Supports real-time streaming of `TaskStatusUpdateEvent` and `TaskArtifactUpdateEvent` via SSE.

### ✅ JSON-RPC 2.0 Transport
All A2A communication uses JSON-RPC 2.0 over HTTP.
- **Root Endpoint:** `/` (Standardized for JSON-RPC POST requests)
- **Methods:** `message/send`, `task/get`, `task/cancel`

### ✅ Message and Part Types
Supports core A2A part types:
- **TextPart:** Plain text content and status logs.
- **FilePart:** References to benchmark results and logs.
- **DataPart:** Structured JSON for evaluation bundles.

### ⚠️ Push Notifications
**Status:** Declared but Not Implemented (Optional). Local benchmark scenarios rely on streaming; webhooks for push notifications are not currently required.

---

## Error Handling

The agents return proper A2A error responses with specific error codes and data fields:

| Error Type | Code | Description |
|------------|------|-------------|
| `InvalidConfigError` | `-32602` | Missing or invalid configuration fields. |
| `TaskNotCancelableError` | `-32001` | Attempting to cancel a task in a terminal state. |
| `ContentTypeNotSupportedError` | `-32002` | Unsupported content type in requests. |

**Implementation:** Error mapping is handled in `smi_bench.a2a_errors` and integrated into the green agent's execution flow.

---

## Testing Strategy

### Philosophy
We ensure protocol compliance through layered testing that validates contract compliance, agent behavior, and integration integrity.

### Test Pyramid
1. **E2E Tests:** Full workflow validation (Few, Slow).
2. **Integration Tests:** Multi-component flow validation (Some, Medium).
3. **Unit Tests:** Individual function validation (Many, Fast).

### Test Categories

| Category | Purpose | Test File |
|----------|---------|-----------|
| **1. Golden Fixtures** | Validate exact JSON structures. | `test_a2a_golden_fixtures.py` |
| **2. HTTP Contracts** | Validate headers, status codes, and JSON-RPC. | `test_a2a_http_contract.py` |
| **3. Protocol Compliance** | Validate against A2A spec requirements. | `test_a2a_protocol_compliance.py` |
| **4. E2E Workflows** | Full task lifecycle (create → stream → cancel). | `test_a2a_e2e_workflows.py` |
| **5. Property-Based** | Generate wide input ranges for robustness. | `test_a2a_property_based.py` |
| **6. Streaming Events** | Validate SSE format and event sequencing. | `test_a2a_streaming_events.py` |
| **7. Schema Validation** | Validate bundles against JSON Schema. | `test_evaluation_bundle_schema.py` |
| **8. Unit/Integration** | Logic, safe conversions, and agent coordination. | `test_a2a_enhancements.py`, `test_integration_a2a.py` |

### Coverage Goals

| Category | Target | Status |
|----------|--------|--------|
| Golden Fixtures | 100% of response types | ✅ Good |
| HTTP Contracts | 100% of endpoints | ✅ Good |
| Protocol Compliance | 100% of spec requirements | ✅ Good |
| E2E Workflows | All critical paths | ✅ Good |
| Unit Tests | >90% code coverage | ✅ Good |
| Property-Based | Critical validation logic | ✅ Good |

---

## Comparison with Spec

| A2A Feature | Spec Section | Status | Notes |
|------------|--------------|--------|-------|
| Agent Card Discovery | 8.2 | ✅ | At `/.well-known/agent-card.json` |
| JSON-RPC 2.0 Transport | 9 | ✅ | Via root `/` endpoint |
| Task Lifecycle | 3.1 | ✅ | All states implemented |
| Send Message | 3.1.1 | ✅ | Returns Task object |
| Streaming | 3.1.2 | ✅ | SSE-based updates |
| Get Task | 3.1.3 | ✅ | Via `TaskStore` |
| Cancel Task | 3.1.5 | ✅ | Graceful termination support |
| Version Headers | 14.2.1 | ✅ | `A2A-Version` middleware |

---

## Future Enhancements

1. **Fuzzing:** Fuzz JSON-RPC requests to discover edge cases.
2. **Performance:** Validate streaming latency and cancellation responsiveness under load.
3. **Compatibility:** Automated testing against multiple A2A protocol versions.
4. **Contract Testing:** Implement consumer-driven contracts (e.g., Pact).

---

## Migration & References

- [A2A Protocol Specification](https://a2a-protocol.org/latest/specification/)
- [Evaluation Bundle Schema](./evaluation_bundle.schema.json)
- [A2A Python SDK](https://github.com/a2aproject/a2a-python)

---

## Changelog

### v0.3.0 (2026-01-03)
- ✨ Added A2A error type mapping with custom error codes.
- ✨ Added property-based testing for configuration validation.
- ✨ Streamlined compliance and testing documentation into a single reference.
- ✅ Fixed HTTP contract endpoints (mapped `/rpc` to `/`).

### v0.2.0 (2026-01-02)
- ✨ Added task cancellation support.
- ✨ Added A2A protocol version headers.
- ✨ Added `protocol_version` field to agent cards.

### v0.1.0 (2025-12-XX)
- Initial A2A protocol implementation (Lifecycle, Discovery, Streaming).
