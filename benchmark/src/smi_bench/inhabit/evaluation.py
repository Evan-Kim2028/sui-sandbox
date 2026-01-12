"""
Evaluation structures for type inhabitation benchmark.

This module provides Python dataclasses that mirror the Rust error taxonomy
from src/benchmark/errors.rs. These structures enable:

1. Phase-based error tracking (Build, Resolution, TypeCheck, Synthesis, Execution, Validation)
2. Error source attribution (LLM error, infrastructure limitation, target limitation)
3. Partial credit scoring with a rubric
4. Inhabitation metrics tracking
5. Execution traces with call stacks and abort info

The structures are designed to be JSON-serializable for benchmark output.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any

# =============================================================================
# Phase-Based Error Taxonomy (v0.5.0)
# =============================================================================


class Phase(str, Enum):
    """Phase of the type inhabitation pipeline.

    The pipeline processes in order:
    Build -> Resolution -> TypeCheck -> Synthesis -> Execution -> Validation
    """

    BUILD = "build"
    RESOLUTION = "resolution"
    TYPECHECK = "typecheck"
    SYNTHESIS = "synthesis"
    EXECUTION = "execution"
    VALIDATION = "validation"

    @property
    def code_prefix(self) -> int:
        """Get the numeric prefix for this phase (0xx, 1xx, 2xx, etc.)"""
        prefixes = {
            Phase.BUILD: 0,
            Phase.RESOLUTION: 100,
            Phase.TYPECHECK: 200,
            Phase.SYNTHESIS: 300,
            Phase.EXECUTION: 400,
            Phase.VALIDATION: 500,
        }
        return prefixes[self]


class ErrorCode(str, Enum):
    """Specific error codes within each phase.

    Error codes are numbered by phase:
    - 0xx: Build errors (pre-pipeline, Move compiler)
    - 1xx: Resolution errors
    - 2xx: Type check errors
    - 3xx: Synthesis errors
    - 4xx: Execution errors
    - 5xx: Validation errors
    """

    # Build Errors (0xx)
    MODULE_ADDRESS_UNDEFINED = "E001"
    INVALID_MANIFEST = "E002"
    IMPORT_RESOLUTION_FAILED = "E003"
    TYPE_SYNTAX_ERROR = "E004"
    INVALID_ENTRY_SIGNATURE = "E005"
    COMPILE_TIME_ABILITY_ERROR = "E006"

    # Resolution Errors (1xx)
    MODULE_NOT_FOUND = "E101"
    FUNCTION_NOT_FOUND = "E102"
    NOT_CALLABLE = "E103"

    # TypeCheck Errors (2xx)
    TYPE_MISMATCH = "E201"
    ABILITY_VIOLATION = "E202"
    GENERIC_BOUNDS_VIOLATION = "E203"
    RECURSIVE_TYPE = "E204"
    UNKNOWN_TYPE = "E205"

    # Synthesis Errors (3xx)
    NO_CONSTRUCTOR = "E301"
    CHAIN_TOO_DEEP = "E302"
    UNSUPPORTED_CONSTRUCTOR_PARAM = "E303"
    BCS_SERIALIZATION_FAILED = "E304"

    # Execution Errors (4xx)
    VM_SETUP_FAILED = "E401"
    CONSTRUCTOR_ABORTED = "E402"
    TARGET_ABORTED = "E403"
    UNSUPPORTED_NATIVE = "E404"

    # Validation Errors (5xx)
    NO_TARGET_MODULES_ACCESSED = "E501"
    RETURN_TYPE_MISMATCH = "E502"

    @property
    def numeric_code(self) -> int:
        """Get the numeric code (e.g., 1, 101, 201, etc.)"""
        return int(self.value[1:])

    @property
    def phase(self) -> Phase:
        """Get the phase this error belongs to."""
        code = self.numeric_code
        if code < 100:
            return Phase.BUILD
        elif code < 200:
            return Phase.RESOLUTION
        elif code < 300:
            return Phase.TYPECHECK
        elif code < 400:
            return Phase.SYNTHESIS
        elif code < 500:
            return Phase.EXECUTION
        else:
            return Phase.VALIDATION

    @property
    def description(self) -> str:
        """Get a short description of this error."""
        descriptions = {
            ErrorCode.MODULE_ADDRESS_UNDEFINED: "module address not defined in Move.toml",
            ErrorCode.INVALID_MANIFEST: "invalid Move.toml syntax",
            ErrorCode.IMPORT_RESOLUTION_FAILED: "import resolution failed (use statement)",
            ErrorCode.TYPE_SYNTAX_ERROR: "type syntax error (qualified path in field)",
            ErrorCode.INVALID_ENTRY_SIGNATURE: "invalid entry function signature",
            ErrorCode.COMPILE_TIME_ABILITY_ERROR: "ability constraint error at compile time",
            ErrorCode.MODULE_NOT_FOUND: "module not found in bytecode corpus",
            ErrorCode.FUNCTION_NOT_FOUND: "function not found in module",
            ErrorCode.NOT_CALLABLE: "function is not public or entry",
            ErrorCode.TYPE_MISMATCH: "argument type does not match parameter type",
            ErrorCode.ABILITY_VIOLATION: "type ability constraint violated",
            ErrorCode.GENERIC_BOUNDS_VIOLATION: "generic type parameter bounds not satisfied",
            ErrorCode.RECURSIVE_TYPE: "recursive type detected",
            ErrorCode.UNKNOWN_TYPE: "unknown type (struct not found)",
            ErrorCode.NO_CONSTRUCTOR: "no constructor found for type",
            ErrorCode.CHAIN_TOO_DEEP: "constructor chain exceeds maximum depth",
            ErrorCode.UNSUPPORTED_CONSTRUCTOR_PARAM: "constructor has unsupported parameter",
            ErrorCode.BCS_SERIALIZATION_FAILED: "BCS serialization failed",
            ErrorCode.VM_SETUP_FAILED: "VM harness setup failed",
            ErrorCode.CONSTRUCTOR_ABORTED: "constructor execution aborted",
            ErrorCode.TARGET_ABORTED: "target function execution aborted",
            ErrorCode.UNSUPPORTED_NATIVE: "unsupported native function called",
            ErrorCode.NO_TARGET_MODULES_ACCESSED: "no target modules accessed",
            ErrorCode.RETURN_TYPE_MISMATCH: "return type mismatch",
        }
        return descriptions.get(self, "unknown error")

    @property
    def is_expected_limitation(self) -> bool:
        """Check if this error represents an expected sandbox limitation."""
        return self in {
            ErrorCode.UNSUPPORTED_NATIVE,
            ErrorCode.CHAIN_TOO_DEEP,
            ErrorCode.UNSUPPORTED_CONSTRUCTOR_PARAM,
        }

    @property
    def default_error_source(self) -> ErrorSource:
        """Get the default error source attribution for this error code."""
        # Build errors are almost always LLM mistakes
        if self.phase == Phase.BUILD:
            return ErrorSource.LLM_ERROR

        # Resolution errors
        if self in {ErrorCode.MODULE_NOT_FOUND, ErrorCode.FUNCTION_NOT_FOUND, ErrorCode.NOT_CALLABLE}:
            return ErrorSource.LLM_ERROR

        # TypeCheck errors
        if self in {
            ErrorCode.TYPE_MISMATCH,
            ErrorCode.ABILITY_VIOLATION,
            ErrorCode.GENERIC_BOUNDS_VIOLATION,
            ErrorCode.UNKNOWN_TYPE,
        }:
            return ErrorSource.LLM_ERROR
        if self == ErrorCode.RECURSIVE_TYPE:
            return ErrorSource.INFRASTRUCTURE_LIMITATION

        # Synthesis errors
        if self == ErrorCode.NO_CONSTRUCTOR:
            return ErrorSource.UNKNOWN  # Context-dependent
        if self in {
            ErrorCode.CHAIN_TOO_DEEP,
            ErrorCode.UNSUPPORTED_CONSTRUCTOR_PARAM,
            ErrorCode.BCS_SERIALIZATION_FAILED,
        }:
            return ErrorSource.INFRASTRUCTURE_LIMITATION

        # Execution errors
        if self == ErrorCode.VM_SETUP_FAILED:
            return ErrorSource.INFRASTRUCTURE_LIMITATION
        if self in {ErrorCode.CONSTRUCTOR_ABORTED, ErrorCode.TARGET_ABORTED}:
            return ErrorSource.UNKNOWN
        if self == ErrorCode.UNSUPPORTED_NATIVE:
            return ErrorSource.INFRASTRUCTURE_LIMITATION

        # Validation errors
        if self in {ErrorCode.NO_TARGET_MODULES_ACCESSED, ErrorCode.RETURN_TYPE_MISMATCH}:
            return ErrorSource.LLM_ERROR

        return ErrorSource.UNKNOWN


class ErrorSource(str, Enum):
    """Attribution for where an error originated."""

    LLM_ERROR = "llm_error"
    INFRASTRUCTURE_LIMITATION = "infrastructure_limitation"
    TARGET_PACKAGE_LIMITATION = "target_package_limitation"
    UNKNOWN = "unknown"

    @property
    def counts_against_llm(self) -> bool:
        """Whether this error should count against the LLM's score."""
        return self == ErrorSource.LLM_ERROR

    @property
    def description(self) -> str:
        """Human-readable description."""
        descriptions = {
            ErrorSource.LLM_ERROR: "LLM generated incorrect code",
            ErrorSource.INFRASTRUCTURE_LIMITATION: "sandbox infrastructure limitation",
            ErrorSource.TARGET_PACKAGE_LIMITATION: "target package has no valid entry points",
            ErrorSource.UNKNOWN: "unknown or ambiguous error source",
        }
        return descriptions.get(self, "unknown")


# =============================================================================
# Failure Context
# =============================================================================


@dataclass
class FailureContext:
    """Additional context for a failure."""

    module: str | None = None
    function: str | None = None
    type_name: str | None = None
    param_index: int | None = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dict, excluding None values."""
        d = {}
        if self.module is not None:
            d["module"] = self.module
        if self.function is not None:
            d["function"] = self.function
        if self.type_name is not None:
            d["type_name"] = self.type_name
        if self.param_index is not None:
            d["param_index"] = self.param_index
        return d


@dataclass
class Failure:
    """Complete failure information for the pipeline."""

    phase: Phase
    code: ErrorCode
    message: str
    is_expected_limitation: bool = False
    error_source: ErrorSource = ErrorSource.UNKNOWN
    context: FailureContext | None = None

    @classmethod
    def from_code(cls, code: ErrorCode, message: str) -> Failure:
        """Create a new failure with just the essentials."""
        return cls(
            phase=code.phase,
            code=code,
            message=message,
            is_expected_limitation=code.is_expected_limitation,
            error_source=code.default_error_source,
        )

    @classmethod
    def with_context(cls, code: ErrorCode, message: str, context: FailureContext) -> Failure:
        """Create a failure with context."""
        return cls(
            phase=code.phase,
            code=code,
            message=message,
            is_expected_limitation=code.is_expected_limitation,
            error_source=code.default_error_source,
            context=context,
        )

    def set_source(self, source: ErrorSource) -> Failure:
        """Set the error source attribution (fluent API)."""
        self.error_source = source
        self.is_expected_limitation = source in {
            ErrorSource.INFRASTRUCTURE_LIMITATION,
            ErrorSource.TARGET_PACKAGE_LIMITATION,
        }
        return self

    def to_dict(self) -> dict[str, Any]:
        """Convert to JSON-serializable dict."""
        d = {
            "phase": self.phase.value,
            "code": self.code.value,
            "message": self.message,
            "is_expected_limitation": self.is_expected_limitation,
            "error_source": self.error_source.value,
        }
        if self.context is not None:
            d["context"] = self.context.to_dict()
        return d


# =============================================================================
# Scoring Rubric (Partial Credit)
# =============================================================================


@dataclass
class ScoringCriteria:
    """Scoring criteria for partial credit evaluation.

    Instead of binary pass/fail, this provides more granular scoring
    to distinguish between models that fail early vs. late in the pipeline.
    """

    compiles: bool = False
    imports_target: bool = False
    creates_target_type: bool = False
    executes_cleanly: bool = False

    def score(self) -> float:
        """Calculate the total score (0.0 to 1.0)"""
        total = 0.0
        if self.compiles:
            total += 0.25
        if self.imports_target:
            total += 0.25
        if self.creates_target_type:
            total += 0.25
        if self.executes_cleanly:
            total += 0.25
        return total

    def phase_reached(self) -> Phase:
        """Get the furthest phase reached in the pipeline."""
        if self.executes_cleanly:
            return Phase.VALIDATION
        elif self.creates_target_type:
            return Phase.EXECUTION
        elif self.imports_target:
            return Phase.SYNTHESIS
        elif self.compiles:
            return Phase.RESOLUTION
        else:
            return Phase.BUILD

    @classmethod
    def from_phase(cls, phase: Phase) -> ScoringCriteria:
        """Create criteria from a phase (for when execution stops at a given phase)."""
        if phase == Phase.BUILD:
            return cls()
        elif phase == Phase.RESOLUTION:
            return cls(compiles=True)
        elif phase in {Phase.TYPECHECK, Phase.SYNTHESIS}:
            return cls(compiles=True, imports_target=True)
        elif phase == Phase.EXECUTION:
            return cls(compiles=True, imports_target=True, creates_target_type=True)
        else:  # Phase.VALIDATION
            return cls(compiles=True, imports_target=True, creates_target_type=True, executes_cleanly=True)

    def to_dict(self) -> dict[str, Any]:
        """Convert to JSON-serializable dict."""
        return {
            "compiles": self.compiles,
            "imports_target": self.imports_target,
            "creates_target_type": self.creates_target_type,
            "executes_cleanly": self.executes_cleanly,
            "score": self.score(),
            "phase_reached": self.phase_reached().value,
        }


# =============================================================================
# Inhabitation Metrics
# =============================================================================


class UninhabitedReason(str, Enum):
    """Reason why a type could not be inhabited."""

    NO_CONSTRUCTOR = "no_constructor"
    UNSUPPORTED_PARAM = "unsupported_param"
    CHAIN_TOO_DEEP = "chain_too_deep"
    RECURSIVE_TYPE = "recursive_type"
    ABILITY_CONSTRAINT = "ability_constraint"
    REQUIRES_RUNTIME_VALUE = "requires_runtime_value"
    UNKNOWN = "unknown"


@dataclass
class UninhabitedType:
    """A type that could not be inhabited, with reason."""

    type_name: str
    reason: UninhabitedReason
    details: str | None = None

    def to_dict(self) -> dict[str, Any]:
        d = {
            "type_name": self.type_name,
            "reason": self.reason.value,
        }
        if self.details is not None:
            d["details"] = self.details
        return d


@dataclass
class InhabitationMetrics:
    """Metrics about type inhabitation (argument synthesis) success.

    Type inhabitation measures how many of a package's public functions can have
    their arguments synthesized automatically. A function is "inhabited" if:
    - All parameter types have valid constructors or default values
    - BCS serialization/deserialization round-trips successfully
    - Type parameters can be instantiated with concrete types

    This is independent of whether the function *executes* successfully -
    that is tracked separately by tier_b_hit status and ScoringCriteria.

    Terminology:
    - target_types_total: Total public/entry functions in the target package
    - target_types_inhabited: Functions where args could be synthesized (tier_a_hit+tier_b_hit)
    - synthesis_rate(): Fraction of functions that could have args synthesized
    - execution_rate(): Fraction of functions that executed successfully (tier_b_hit only)
    """

    target_types_total: int = 0
    target_types_inhabited: int = 0
    inhabited_types: list[str] = field(default_factory=list)
    uninhabited_types: list[UninhabitedType] = field(default_factory=list)
    target_entry_functions: int = 0
    entry_functions_called: int = 0
    stdlib_types_used: list[str] = field(default_factory=list)

    def inhabitation_rate(self) -> float:
        """Calculate argument synthesis success rate (0.0 to 1.0).

        This is the fraction of target package functions where arguments could be
        synthesized automatically. A rate of 0.4 means 40% of functions had their
        parameters successfully inhabited.

        Also available as synthesis_rate() for clearer naming.
        """
        if self.target_types_total == 0:
            return 0.0
        return self.target_types_inhabited / self.target_types_total

    def synthesis_rate(self) -> float:
        """Alias for inhabitation_rate() - fraction of functions with synthesizable args."""
        return self.inhabitation_rate()

    def entry_coverage(self) -> float:
        """Calculate entry function execution rate (0.0 to 1.0).

        This is the fraction of entry functions that were successfully executed
        (tier_b_hit with execution_success=True).
        """
        if self.target_entry_functions == 0:
            return 0.0
        return self.entry_functions_called / self.target_entry_functions

    def to_dict(self) -> dict[str, Any]:
        d = {
            "target_types_total": self.target_types_total,
            "target_types_inhabited": self.target_types_inhabited,
            "inhabitation_rate": self.inhabitation_rate(),
            "entry_coverage": self.entry_coverage(),
            "target_entry_functions": self.target_entry_functions,
            "entry_functions_called": self.entry_functions_called,
        }
        if self.inhabited_types:
            d["inhabited_types"] = self.inhabited_types
        if self.uninhabited_types:
            d["uninhabited_types"] = [u.to_dict() for u in self.uninhabited_types]
        if self.stdlib_types_used:
            d["stdlib_types_used"] = self.stdlib_types_used
        return d


# =============================================================================
# Execution Trace
# =============================================================================


class AbortCategory(str, Enum):
    """Category of abort for easier analysis."""

    UNSUPPORTED_NATIVE = "unsupported_native"
    ASSERTION_FAILED = "assertion_failed"
    OBJECT_ERROR = "object_error"
    ARITHMETIC_ERROR = "arithmetic_error"
    VECTOR_BOUNDS_ERROR = "vector_bounds_error"
    TYPE_ERROR = "type_error"
    OUT_OF_GAS = "out_of_gas"
    UNKNOWN = "unknown"


E_NOT_SUPPORTED = 1000  # Abort code for unsupported natives


@dataclass
class StackFrame:
    """A frame in the call stack."""

    module: str
    function: str
    instruction_offset: int | None = None

    def to_dict(self) -> dict[str, Any]:
        d = {
            "module": self.module,
            "function": self.function,
        }
        if self.instruction_offset is not None:
            d["instruction_offset"] = self.instruction_offset
        return d


@dataclass
class AbortInfo:
    """Detailed information about an abort during execution."""

    message: str
    is_expected: bool
    category: AbortCategory
    abort_code: int | None = None
    abort_location: str | None = None
    call_stack: list[StackFrame] = field(default_factory=list)

    @classmethod
    def from_move_abort(cls, code: int, location: str | None, message: str) -> AbortInfo:
        """Create abort info from a MoveAbort error."""
        is_expected = code == E_NOT_SUPPORTED
        category = cls._categorize_abort(code, message)
        return cls(
            message=message,
            is_expected=is_expected,
            category=category,
            abort_code=code,
            abort_location=location,
        )

    @staticmethod
    def _categorize_abort(code: int, message: str) -> AbortCategory:
        """Categorize an abort based on code and message."""
        if code == E_NOT_SUPPORTED:
            return AbortCategory.UNSUPPORTED_NATIVE

        msg_lower = message.lower()
        if "assert" in msg_lower:
            return AbortCategory.ASSERTION_FAILED
        elif "object" in msg_lower or "ownership" in msg_lower:
            return AbortCategory.OBJECT_ERROR
        elif "overflow" in msg_lower or "underflow" in msg_lower or "divide" in msg_lower:
            return AbortCategory.ARITHMETIC_ERROR
        elif "vector" in msg_lower or "index" in msg_lower:
            return AbortCategory.VECTOR_BOUNDS_ERROR
        elif "type" in msg_lower or "ability" in msg_lower:
            return AbortCategory.TYPE_ERROR
        elif "gas" in msg_lower:
            return AbortCategory.OUT_OF_GAS
        return AbortCategory.UNKNOWN

    def push_frame(self, module: str, function: str, offset: int | None = None) -> None:
        """Add a stack frame to the call stack."""
        self.call_stack.append(StackFrame(module=module, function=function, instruction_offset=offset))

    def to_dict(self) -> dict[str, Any]:
        d = {
            "message": self.message,
            "is_expected": self.is_expected,
            "category": self.category.value,
        }
        if self.abort_code is not None:
            d["abort_code"] = self.abort_code
        if self.abort_location is not None:
            d["abort_location"] = self.abort_location
        if self.call_stack:
            d["call_stack"] = [f.to_dict() for f in self.call_stack]
        return d


@dataclass
class FunctionCall:
    """Information about a function call during execution."""

    module: str
    function: str
    type_args: list[str] = field(default_factory=list)
    succeeded: bool = True
    error: str | None = None

    def to_dict(self) -> dict[str, Any]:
        d = {
            "module": self.module,
            "function": self.function,
            "succeeded": self.succeeded,
        }
        if self.type_args:
            d["type_args"] = self.type_args
        if self.error is not None:
            d["error"] = self.error
        return d


@dataclass
class ExecutionTrace:
    """Execution trace for debugging and analysis."""

    execution_attempted: bool = False
    modules_loaded: list[str] = field(default_factory=list)
    functions_called: list[FunctionCall] = field(default_factory=list)
    abort_info: AbortInfo | None = None
    gas_used: int | None = None
    duration_ms: int | None = None

    def record_call(self, module: str, function: str, type_args: list[str] | None = None) -> None:
        """Record a function call."""
        self.functions_called.append(
            FunctionCall(
                module=module,
                function=function,
                type_args=type_args or [],
                succeeded=True,
            )
        )

    def mark_last_failed(self, error: str) -> None:
        """Mark the last call as failed."""
        if self.functions_called:
            self.functions_called[-1].succeeded = False
            self.functions_called[-1].error = error

    def to_dict(self) -> dict[str, Any]:
        d = {
            "execution_attempted": self.execution_attempted,
        }
        if self.modules_loaded:
            d["modules_loaded"] = self.modules_loaded
        if self.functions_called:
            d["functions_called"] = [c.to_dict() for c in self.functions_called]
        if self.abort_info is not None:
            d["abort_info"] = self.abort_info.to_dict()
        if self.gas_used is not None:
            d["gas_used"] = self.gas_used
        if self.duration_ms is not None:
            d["duration_ms"] = self.duration_ms
        return d


# =============================================================================
# Complete Evaluation Result
# =============================================================================


@dataclass
class EvaluationResult:
    """Complete evaluation result with scoring."""

    ok: bool
    score: float
    criteria: ScoringCriteria
    failure: Failure | None = None
    partial_credit_reason: str | None = None
    inhabitation_metrics: InhabitationMetrics | None = None
    execution_trace: ExecutionTrace | None = None

    @classmethod
    def success(cls) -> EvaluationResult:
        """Create a successful result."""
        return cls(
            ok=True,
            score=1.0,
            criteria=ScoringCriteria(
                compiles=True,
                imports_target=True,
                creates_target_type=True,
                executes_cleanly=True,
            ),
        )

    @classmethod
    def success_with_details(
        cls,
        metrics: InhabitationMetrics,
        trace: ExecutionTrace,
    ) -> EvaluationResult:
        """Create a successful result with metrics and trace."""
        return cls(
            ok=True,
            score=1.0,
            criteria=ScoringCriteria(
                compiles=True,
                imports_target=True,
                creates_target_type=True,
                executes_cleanly=True,
            ),
            inhabitation_metrics=metrics,
            execution_trace=trace,
        )

    @classmethod
    def failed(cls, failure: Failure) -> EvaluationResult:
        """Create a failed result with partial credit."""
        criteria = ScoringCriteria.from_phase(failure.phase)
        score = criteria.score()
        phase_reached = criteria.phase_reached()

        partial_credit_reason = None
        if score > 0.0:
            partial_credit_reason = f"Reached {phase_reached.value} phase before failure"

        return cls(
            ok=False,
            score=score,
            criteria=criteria,
            failure=failure,
            partial_credit_reason=partial_credit_reason,
        )

    @classmethod
    def failed_with_details(
        cls,
        failure: Failure,
        criteria: ScoringCriteria,
        metrics: InhabitationMetrics | None = None,
        trace: ExecutionTrace | None = None,
    ) -> EvaluationResult:
        """Create a failed result with full details."""
        score = criteria.score()
        phase_reached = criteria.phase_reached()

        partial_credit_reason = None
        if score > 0.0:
            partial_credit_reason = f"Reached {phase_reached.value} phase before failure"

        return cls(
            ok=False,
            score=score,
            criteria=criteria,
            failure=failure,
            partial_credit_reason=partial_credit_reason,
            inhabitation_metrics=metrics,
            execution_trace=trace,
        )

    def with_metrics(self, metrics: InhabitationMetrics) -> EvaluationResult:
        """Add inhabitation metrics to an existing result."""
        self.inhabitation_metrics = metrics
        return self

    def with_trace(self, trace: ExecutionTrace) -> EvaluationResult:
        """Add execution trace to an existing result."""
        self.execution_trace = trace
        return self

    def to_dict(self) -> dict[str, Any]:
        """Convert to JSON-serializable dict."""
        d = {
            "ok": self.ok,
            "score": self.score,
            "criteria": self.criteria.to_dict(),
        }
        if self.failure is not None:
            d["failure"] = self.failure.to_dict()
        if self.partial_credit_reason is not None:
            d["partial_credit_reason"] = self.partial_credit_reason
        if self.inhabitation_metrics is not None:
            d["inhabitation_metrics"] = self.inhabitation_metrics.to_dict()
        if self.execution_trace is not None:
            d["execution_trace"] = self.execution_trace.to_dict()
        return d


# =============================================================================
# Helper Functions
# =============================================================================


def parse_build_error(error_text: str) -> ErrorCode | None:
    """Parse Move compiler error text and return the appropriate ErrorCode.

    This maps common Move/Sui compiler error patterns to our error taxonomy.
    """
    error_lower = error_text.lower()

    # E001: Address with no value (E03001)
    if "e03001" in error_lower or "address with no value" in error_lower:
        return ErrorCode.MODULE_ADDRESS_UNDEFINED

    # E002: Invalid Move.toml
    if "move.toml" in error_lower and ("parse" in error_lower or "invalid" in error_lower):
        return ErrorCode.INVALID_MANIFEST

    # E003: Import resolution failed
    if "e04001" in error_lower or "unbound module" in error_lower:
        return ErrorCode.IMPORT_RESOLUTION_FAILED

    # E004: Type syntax error (E03006)
    if "e03006" in error_lower or "invalid qualified path" in error_lower:
        return ErrorCode.TYPE_SYNTAX_ERROR

    # E005: Invalid entry function signature (Sui E02002)
    if "sui e02002" in error_lower or "invalid 'entry' function signature" in error_lower:
        return ErrorCode.INVALID_ENTRY_SIGNATURE

    # E006: Ability constraint error
    if "ability constraint" in error_lower or "missing ability" in error_lower:
        return ErrorCode.COMPILE_TIME_ABILITY_ERROR

    return None


def is_unsupported_native_error(error: str) -> bool:
    """Check if an error message indicates an unsupported native function."""
    return str(E_NOT_SUPPORTED) in error and "MoveAbort" in error
