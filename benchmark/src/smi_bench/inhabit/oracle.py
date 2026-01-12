"""
Oracle/Ceiling computation and difficulty ranking for type inhabitation.

This module provides:
1. PackageOracle: Computes the theoretical maximum achievable score for a package
   by analyzing the MM2 target mapping (which runs benchmark-local directly on
   the target package without LLM involvement).

2. FunctionDifficulty: Ranks functions by difficulty based on parameter complexity,
   constructor requirements, and execution success history.

3. RankedInterfaceSummary: Provides a difficulty-ranked interface summary for LLM
   prompts, prioritizing harder functions to challenge the model.

The oracle answers: "What is the maximum possible score for this package?"
The difficulty ranking answers: "Which functions should we prioritize for testing?"
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any


class DifficultyLevel(str, Enum):
    """Difficulty level for a function."""
    TRIVIAL = "trivial"           # No params or only primitives
    SIMPLE = "simple"             # Primitives + simple vectors
    MODERATE = "moderate"         # System objects (Clock, TxContext)
    COMPLEX = "complex"           # Custom types with constructors
    CHALLENGING = "challenging"   # Type parameters or deep chains
    DIFFICULT = "difficult"       # Multiple complex requirements
    VERY_DIFFICULT = "very_difficult"  # Object params, no execution possible
    IMPOSSIBLE = "impossible"     # No synthesis path exists


@dataclass
class FunctionDifficulty:
    """Difficulty assessment for a single function."""
    module: str
    function: str
    level: DifficultyLevel
    score: float  # 0.0 (easiest) to 1.0 (hardest)

    # Contributing factors
    param_count: int = 0
    has_type_params: bool = False
    has_object_params: bool = False
    constructor_depth: int = 0
    requires_synthesis: list[str] = field(default_factory=list)  # e.g., ["TxContext", "Clock"]
    # NEW: Track constructible references (refs to types we can construct via constructor chain)
    constructible_ref_types: list[str] = field(default_factory=list)  # e.g., ["&FeeConfig", "&mut Cell"]
    resolved_params: list[str] = field(default_factory=list)  # Full list of resolved params for logging

    # Oracle results (if available)
    oracle_tier_a: bool = False  # Can args be synthesized?
    oracle_tier_b: bool = False  # Does it execute successfully?
    oracle_failure_reason: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "module": self.module,
            "function": self.function,
            "level": self.level.value,
            "score": self.score,
            "param_count": self.param_count,
            "has_type_params": self.has_type_params,
            "has_object_params": self.has_object_params,
            "constructor_depth": self.constructor_depth,
            "requires_synthesis": self.requires_synthesis,
            "constructible_ref_types": self.constructible_ref_types,
            "resolved_params": self.resolved_params,
            "oracle_tier_a": self.oracle_tier_a,
            "oracle_tier_b": self.oracle_tier_b,
            "oracle_failure_reason": self.oracle_failure_reason,
        }


@dataclass
class PackageOracle:
    """Oracle/ceiling for a package's maximum achievable score.

    This is computed from mm2_target_mapping.json, which runs benchmark-local
    directly on the target package bytecode. It tells us what's theoretically
    achievable without any LLM involvement.

    Key insight: If the oracle shows a function can't be synthesized/executed,
    then the LLM can't be blamed for failing on it.
    """
    package_id: str
    total_functions: int

    # Tier A: Synthesis success (args can be synthesized)
    tier_a_possible: int  # Functions where synthesis succeeded
    tier_a_impossible: int  # Functions where synthesis failed

    # Tier B: Execution success (function runs without abort)
    tier_b_possible: int  # Functions that executed successfully
    tier_b_blocked_by_infra: int  # Would succeed but blocked by infra (object params)
    tier_b_failed: int  # Synthesis succeeded but execution aborted

    # Maximum achievable scores
    max_synthesis_rate: float  # tier_a_possible / total_functions
    max_execution_rate: float  # tier_b_possible / total_functions

    # Ceiling explanation
    impossible_functions: list[dict[str, Any]] = field(default_factory=list)
    infra_blocked_functions: list[dict[str, Any]] = field(default_factory=list)

    # Difficulty distribution
    difficulty_distribution: dict[str, int] = field(default_factory=dict)
    functions_by_difficulty: list[FunctionDifficulty] = field(default_factory=list)

    @classmethod
    def from_mm2_target_mapping(
        cls,
        mm2_path: Path | str,
        package_id: str | None = None,
    ) -> PackageOracle:
        """Compute oracle from mm2_target_mapping.json."""
        mm2_path = Path(mm2_path)
        data = json.loads(mm2_path.read_text(encoding="utf-8"))

        accepted = data.get("accepted", [])
        rejected = data.get("rejected", [])

        # Extract package ID from first entry if not provided
        if package_id is None:
            for entry in accepted + rejected:
                pkg = entry.get("target_package", "")
                if pkg and not pkg.startswith("0x0"):
                    package_id = pkg
                    break
            package_id = package_id or "unknown"

        total = len(accepted) + len(rejected)

        tier_a_possible = 0
        tier_b_possible = 0
        tier_b_blocked = 0
        tier_b_failed = 0

        impossible: list[dict[str, Any]] = []
        infra_blocked: list[dict[str, Any]] = []
        functions_diff: list[FunctionDifficulty] = []

        # Process accepted entries
        for entry in accepted:
            status = entry.get("status", "")
            tier_a = entry.get("tier_a_details", {})
            tier_b = entry.get("tier_b_details", {})

            module = entry.get("target_module", "")
            func = entry.get("target_function", "")

            if status in ("tier_a_hit", "tier_b_hit"):
                tier_a_possible += 1

            if status == "tier_b_hit":
                tier_b_possible += 1
            elif status == "tier_a_hit":
                # Check if blocked by object params (infra limitation)
                if tier_a.get("has_object_params"):
                    tier_b_blocked += 1
                    infra_blocked.append({
                        "module": module,
                        "function": func,
                        "reason": "requires object params (infra limitation)",
                        "resolved_params": tier_a.get("resolved_params", []),
                    })
                else:
                    # Synthesis succeeded but execution failed
                    tier_b_failed += 1

            # Compute difficulty
            diff = _compute_function_difficulty(entry)
            functions_diff.append(diff)

        # Process rejected entries
        for entry in rejected:
            module = entry.get("target_module", "")
            func = entry.get("target_function", "")
            reason = entry.get("failure_reason", "unknown")
            stage = entry.get("failure_stage", "")

            impossible.append({
                "module": module,
                "function": func,
                "reason": reason,
                "failure_stage": stage,
            })

            # Mark as impossible difficulty
            diff = FunctionDifficulty(
                module=module,
                function=func,
                level=DifficultyLevel.IMPOSSIBLE,
                score=1.0,
                oracle_tier_a=False,
                oracle_tier_b=False,
                oracle_failure_reason=reason,
            )
            functions_diff.append(diff)

        tier_a_impossible = len(rejected)

        # Calculate rates
        max_synthesis = tier_a_possible / total if total > 0 else 0.0
        max_execution = tier_b_possible / total if total > 0 else 0.0

        # Sort by difficulty (hardest first)
        functions_diff.sort(key=lambda f: -f.score)

        # Count difficulty distribution
        distribution: dict[str, int] = {}
        for diff in functions_diff:
            level = diff.level.value
            distribution[level] = distribution.get(level, 0) + 1

        return cls(
            package_id=package_id,
            total_functions=total,
            tier_a_possible=tier_a_possible,
            tier_a_impossible=tier_a_impossible,
            tier_b_possible=tier_b_possible,
            tier_b_blocked_by_infra=tier_b_blocked,
            tier_b_failed=tier_b_failed,
            max_synthesis_rate=max_synthesis,
            max_execution_rate=max_execution,
            impossible_functions=impossible,
            infra_blocked_functions=infra_blocked,
            difficulty_distribution=distribution,
            functions_by_difficulty=functions_diff,
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "package_id": self.package_id,
            "total_functions": self.total_functions,
            "tier_a_possible": self.tier_a_possible,
            "tier_a_impossible": self.tier_a_impossible,
            "tier_b_possible": self.tier_b_possible,
            "tier_b_blocked_by_infra": self.tier_b_blocked_by_infra,
            "tier_b_failed": self.tier_b_failed,
            "max_synthesis_rate": round(self.max_synthesis_rate, 4),
            "max_execution_rate": round(self.max_execution_rate, 4),
            "difficulty_distribution": self.difficulty_distribution,
            "impossible_functions": self.impossible_functions,
            "infra_blocked_functions": self.infra_blocked_functions,
        }

    def summary(self) -> str:
        """Generate a human-readable summary."""
        lines = [
            f"Package Oracle: {self.package_id}",
            f"  Total functions: {self.total_functions}",
            "",
            "  Synthesis (Tier A):",
            f"    Possible: {self.tier_a_possible} ({self.max_synthesis_rate:.1%})",
            f"    Impossible: {self.tier_a_impossible}",
            "",
            "  Execution (Tier B):",
            f"    Possible: {self.tier_b_possible} ({self.max_execution_rate:.1%})",
            f"    Blocked by infra: {self.tier_b_blocked_by_infra}",
            f"    Failed: {self.tier_b_failed}",
            "",
            "  Difficulty distribution:",
        ]
        for level in DifficultyLevel:
            count = self.difficulty_distribution.get(level.value, 0)
            if count > 0:
                lines.append(f"    {level.value}: {count}")

        return "\n".join(lines)

    def get_functions_by_difficulty(
        self,
        min_level: DifficultyLevel = DifficultyLevel.TRIVIAL,
        max_level: DifficultyLevel = DifficultyLevel.IMPOSSIBLE,
        limit: int | None = None,
    ) -> list[FunctionDifficulty]:
        """Get functions filtered and sorted by difficulty."""
        # Difficulty order for comparison
        order = list(DifficultyLevel)
        min_idx = order.index(min_level)
        max_idx = order.index(max_level)

        filtered = [
            f for f in self.functions_by_difficulty
            if min_idx <= order.index(f.level) <= max_idx
        ]

        if limit is not None:
            filtered = filtered[:limit]

        return filtered


def _compute_function_difficulty(entry: dict[str, Any]) -> FunctionDifficulty:
    """Compute difficulty for a single MM2 entry."""
    module = entry.get("target_module", "")
    func = entry.get("target_function", "")
    status = entry.get("status", "miss")
    tier_a = entry.get("tier_a_details", {})
    tier_b = entry.get("tier_b_details", {})
    failure_reason = entry.get("failure_reason")

    resolved_params = tier_a.get("resolved_params", [])
    has_object_params = tier_a.get("has_object_params", False)

    # Count parameters and analyze types
    param_count = len(resolved_params)
    has_type_params = any("type_param" in p for p in resolved_params)
    has_construct = any("construct:" in p for p in resolved_params)
    # Multi-hop patterns: construct_hop, construct_hop2, construct_hop3, etc.
    has_construct_hop = any("construct_hop" in p for p in resolved_params)
    # NEW: Track constructible references (references to types we can construct)
    has_constructible_ref = any("constructible_ref:" in p for p in resolved_params)
    # Multi-hop ref patterns: constructible_ref_hop, constructible_ref_hop2, etc.
    has_constructible_ref_hop = any("constructible_ref_hop" in p for p in resolved_params)
    requires_synthesis = [
        p.split(":")[1] for p in resolved_params if p.startswith("synthesizable:")
    ]
    # Extract constructible ref types for logging
    constructible_ref_types = [
        p.split(":")[-1] for p in resolved_params
        if "constructible_ref" in p
    ]

    # Calculate constructor depth from resolved_params patterns
    # construct_hopN or constructible_ref_hopN where N is the depth
    constructor_depth = 0
    if has_construct or has_constructible_ref:
        constructor_depth = 1
    if has_construct_hop or has_constructible_ref_hop:
        # Parse the actual hop depth from patterns like "construct_hop2:" or "constructible_ref_hop3:"
        for p in resolved_params:
            if "construct_hop" in p or "constructible_ref_hop" in p:
                # Extract number from patterns like "construct_hop2:TypeName"
                match = re.search(r'(?:construct_hop|constructible_ref_hop)(\d+)?:', p)
                if match:
                    depth_str = match.group(1)
                    if depth_str:
                        depth = int(depth_str)
                    else:
                        depth = 2  # Default to 2 for "construct_hop:" without number
                    constructor_depth = max(constructor_depth, depth)

    # Score calculation
    score = 0.0

    # Base score from param count (0.0 - 0.2)
    score += min(0.2, param_count * 0.04)

    # Type params add significant difficulty (0.15)
    if has_type_params:
        score += 0.15

    # Object params make execution impossible in sandbox (0.2)
    if has_object_params:
        score += 0.2

    # Constructor requirements (0.0 - 0.2)
    score += constructor_depth * 0.1

    # Synthesizable params are relatively easy (0.05 each)
    score += len(requires_synthesis) * 0.05

    # Execution failure adds difficulty (0.1)
    if status == "tier_a_hit" and tier_b.get("execution_success") is False:
        score += 0.1

    # Determine level
    if status == "miss":
        level = DifficultyLevel.IMPOSSIBLE
        score = 1.0
    elif score < 0.1:
        level = DifficultyLevel.TRIVIAL
    elif score < 0.2:
        level = DifficultyLevel.SIMPLE
    elif score < 0.3:
        level = DifficultyLevel.MODERATE
    elif score < 0.45:
        level = DifficultyLevel.COMPLEX
    elif score < 0.6:
        level = DifficultyLevel.CHALLENGING
    elif score < 0.8:
        level = DifficultyLevel.DIFFICULT
    else:
        level = DifficultyLevel.VERY_DIFFICULT

    return FunctionDifficulty(
        module=module,
        function=func,
        level=level,
        score=min(1.0, score),
        param_count=param_count,
        has_type_params=has_type_params,
        has_object_params=has_object_params,
        constructor_depth=constructor_depth,
        requires_synthesis=requires_synthesis,
        constructible_ref_types=constructible_ref_types,
        resolved_params=resolved_params,
        oracle_tier_a=status in ("tier_a_hit", "tier_b_hit"),
        oracle_tier_b=status == "tier_b_hit",
        oracle_failure_reason=failure_reason,
    )


def compute_oracle_from_run_dir(run_dir: Path) -> PackageOracle | None:
    """Compute oracle from a benchmark run directory.

    Looks for mm2_target_mapping.json which contains the results of running
    benchmark-local directly on the target package.
    """
    mm2_path = run_dir / "mm2_target_mapping.json"
    if not mm2_path.exists():
        return None

    # Try to get package ID from run_config
    package_id = None
    config_path = run_dir / "run_config.json"
    if config_path.exists():
        try:
            config = json.loads(config_path.read_text(encoding="utf-8"))
            package_id = config.get("package_id")
        except Exception:
            pass

    return PackageOracle.from_mm2_target_mapping(mm2_path, package_id)


@dataclass
class LLMScores:
    """LLM performance scores relative to ceiling.

    These scores are normalized 0-100% values that answer:
    - execution_score: Of the functions that COULD execute, what % did the LLM get?
    - synthesis_score: Of the functions that COULD be synthesized, what % did the LLM get?

    These are comparable across packages and LLM runs.
    """
    # Raw counts from LLM result (target package functions only)
    llm_tier_b_hits: int  # Functions that executed successfully
    llm_tier_a_hits: int  # Functions synthesized but not executed
    llm_total_accepted: int  # tier_b + tier_a
    llm_rejected: int  # Functions the LLM failed to call

    # Ceiling values (from oracle)
    ceiling_tier_b: int  # Max possible tier_b hits
    ceiling_tier_a: int  # Max possible tier_a only hits
    ceiling_total: int  # Total synthesizable functions

    # Normalized scores (0.0 - 1.0)
    execution_score: float  # llm_tier_b / ceiling_tier_b
    synthesis_score: float  # llm_accepted / ceiling_total

    def to_dict(self) -> dict[str, Any]:
        return {
            "llm_tier_b_hits": self.llm_tier_b_hits,
            "llm_tier_a_hits": self.llm_tier_a_hits,
            "llm_total_accepted": self.llm_total_accepted,
            "llm_rejected": self.llm_rejected,
            "ceiling_tier_b": self.ceiling_tier_b,
            "ceiling_tier_a": self.ceiling_tier_a,
            "ceiling_total": self.ceiling_total,
            "execution_score": round(self.execution_score, 4),
            "synthesis_score": round(self.synthesis_score, 4),
            "execution_score_pct": f"{self.execution_score * 100:.1f}%",
            "synthesis_score_pct": f"{self.synthesis_score * 100:.1f}%",
        }

    def summary(self) -> str:
        return (
            f"Execution: {self.execution_score * 100:.1f}% ({self.llm_tier_b_hits}/{self.ceiling_tier_b}), "
            f"Synthesis: {self.synthesis_score * 100:.1f}% ({self.llm_total_accepted}/{self.ceiling_total})"
        )


def compute_llm_scores(
    oracle: PackageOracle,
    mm2_combined_path: Path | str,
    target_package_id: str | None = None,
) -> LLMScores:
    """Compute LLM scores relative to the oracle ceiling.

    Args:
        oracle: PackageOracle with ceiling values
        mm2_combined_path: Path to mm2_combined_mapping.json (LLM result)
        target_package_id: Target package ID to filter by (uses oracle if not provided)

    Returns:
        LLMScores with normalized 0-100% scores
    """
    mm2_path = Path(mm2_combined_path)
    data = json.loads(mm2_path.read_text(encoding="utf-8"))

    target_pkg = target_package_id or oracle.package_id

    def is_target_pkg(entry: dict[str, Any]) -> bool:
        """Check if entry is from target package (not helper)."""
        pkg = entry.get("target_package", "")
        if not pkg or pkg.startswith("0x0"):
            return False
        # Match full ID or last 16 chars (short form)
        return pkg == target_pkg or (
            len(target_pkg) >= 16 and pkg.endswith(target_pkg[-16:])
        )

    accepted = data.get("accepted", [])
    rejected = data.get("rejected", [])

    # Count LLM results for TARGET package only
    llm_tier_b = len([
        x for x in accepted
        if x.get("status") == "tier_b_hit" and is_target_pkg(x)
    ])
    llm_tier_a = len([
        x for x in accepted
        if x.get("status") == "tier_a_hit" and is_target_pkg(x)
    ])
    llm_rejected = len([x for x in rejected if is_target_pkg(x)])

    llm_total = llm_tier_b + llm_tier_a

    # Ceiling from oracle
    ceiling_tier_b = oracle.tier_b_possible
    ceiling_total = oracle.tier_a_possible  # All synthesizable functions
    ceiling_tier_a = ceiling_total - ceiling_tier_b  # tier_a only (not tier_b)

    # Compute normalized scores
    execution_score = llm_tier_b / ceiling_tier_b if ceiling_tier_b > 0 else 0.0
    synthesis_score = llm_total / ceiling_total if ceiling_total > 0 else 0.0

    return LLMScores(
        llm_tier_b_hits=llm_tier_b,
        llm_tier_a_hits=llm_tier_a,
        llm_total_accepted=llm_total,
        llm_rejected=llm_rejected,
        ceiling_tier_b=ceiling_tier_b,
        ceiling_tier_a=ceiling_tier_a,
        ceiling_total=ceiling_total,
        execution_score=min(1.0, execution_score),  # Cap at 100%
        synthesis_score=min(1.0, synthesis_score),
    )


def rank_functions_for_llm(
    oracle: PackageOracle,
    strategy: str = "hardest_first",
    limit: int = 30,
    exclude_impossible: bool = True,
) -> list[FunctionDifficulty]:
    """Rank functions for LLM interface summary.

    Strategies:
    - "hardest_first": Prioritize most difficult (but possible) functions
    - "easiest_first": Prioritize easiest functions (current default behavior)
    - "balanced": Mix of difficulty levels
    - "executable_only": Only functions that can actually execute (tier_b)

    Args:
        oracle: PackageOracle with difficulty analysis
        strategy: Ranking strategy
        limit: Maximum functions to return
        exclude_impossible: Whether to exclude functions that can't be synthesized
    """
    functions = oracle.functions_by_difficulty.copy()

    if exclude_impossible:
        functions = [f for f in functions if f.level != DifficultyLevel.IMPOSSIBLE]

    if strategy == "hardest_first":
        # Already sorted hardest first
        pass
    elif strategy == "easiest_first":
        functions.sort(key=lambda f: f.score)
    elif strategy == "balanced":
        # Interleave difficulty levels
        by_level: dict[DifficultyLevel, list[FunctionDifficulty]] = {}
        for f in functions:
            by_level.setdefault(f.level, []).append(f)

        # Round-robin from each level
        result = []
        levels = [l for l in DifficultyLevel if l in by_level]
        while len(result) < limit and any(by_level.values()):
            for level in levels:
                if by_level.get(level):
                    result.append(by_level[level].pop(0))
                    if len(result) >= limit:
                        break
        functions = result
    elif strategy == "executable_only":
        functions = [f for f in functions if f.oracle_tier_b]
        functions.sort(key=lambda f: -f.score)  # Hardest executable first

    return functions[:limit]
