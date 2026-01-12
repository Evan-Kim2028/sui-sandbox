"""Tests for the oracle module."""

import json
import tempfile

import pytest

from smi_bench.inhabit.oracle import (
    DifficultyLevel,
    FunctionDifficulty,
    PackageOracle,
    _compute_function_difficulty,
    compute_oracle_from_run_dir,
    rank_functions_for_llm,
)


class TestFunctionDifficulty:
    """Tests for FunctionDifficulty dataclass."""

    def test_to_dict(self):
        diff = FunctionDifficulty(
            module="test",
            function="foo",
            level=DifficultyLevel.SIMPLE,
            score=0.15,
            param_count=2,
            has_type_params=False,
            has_object_params=False,
        )
        d = diff.to_dict()
        assert d["module"] == "test"
        assert d["function"] == "foo"
        assert d["level"] == "simple"
        assert d["score"] == 0.15


class TestComputeFunctionDifficulty:
    """Tests for _compute_function_difficulty."""

    def test_trivial_function(self):
        entry = {
            "status": "tier_b_hit",
            "target_module": "test",
            "target_function": "simple_fn",
            "tier_a_details": {
                "resolved_params": [],
                "has_object_params": False,
            },
            "tier_b_details": {"execution_success": True},
        }
        diff = _compute_function_difficulty(entry)
        assert diff.level == DifficultyLevel.TRIVIAL
        assert diff.score < 0.1
        assert diff.oracle_tier_b is True

    def test_function_with_primitives(self):
        entry = {
            "status": "tier_b_hit",
            "target_module": "test",
            "target_function": "with_primitives",
            "tier_a_details": {
                "resolved_params": ["U64", "Bool"],
                "has_object_params": False,
            },
            "tier_b_details": {"execution_success": True},
        }
        diff = _compute_function_difficulty(entry)
        assert diff.param_count == 2
        assert diff.level in (DifficultyLevel.TRIVIAL, DifficultyLevel.SIMPLE)

    def test_function_with_type_params(self):
        entry = {
            "status": "tier_b_hit",
            "target_module": "test",
            "target_function": "generic_fn",
            "tier_a_details": {
                "resolved_params": ["type_param[0]=U64"],
                "has_object_params": False,
            },
            "tier_b_details": {"execution_success": True},
        }
        diff = _compute_function_difficulty(entry)
        assert diff.has_type_params is True
        assert diff.score >= 0.15  # Type params add difficulty

    def test_function_with_object_params(self):
        entry = {
            "status": "tier_a_hit",
            "target_module": "test",
            "target_function": "needs_object",
            "tier_a_details": {
                "resolved_params": ["object"],
                "has_object_params": True,
            },
        }
        diff = _compute_function_difficulty(entry)
        assert diff.has_object_params is True
        assert diff.oracle_tier_b is False  # Can't execute with object params

    def test_function_with_constructor(self):
        entry = {
            "status": "tier_b_hit",
            "target_module": "test",
            "target_function": "needs_construct",
            "tier_a_details": {
                "resolved_params": ["construct:MyType"],
                "has_object_params": False,
            },
            "tier_b_details": {"execution_success": True},
        }
        diff = _compute_function_difficulty(entry)
        assert diff.constructor_depth == 1

    def test_function_with_constructor_hop(self):
        entry = {
            "status": "tier_b_hit",
            "target_module": "test",
            "target_function": "needs_hop",
            "tier_a_details": {
                "resolved_params": ["construct_hop:Config"],
                "has_object_params": False,
            },
            "tier_b_details": {"execution_success": True},
        }
        diff = _compute_function_difficulty(entry)
        assert diff.constructor_depth == 2

    def test_impossible_function(self):
        entry = {
            "status": "miss",
            "target_module": "test",
            "target_function": "impossible_fn",
            "failure_reason": "no constructor",
            "failure_stage": "A3",
        }
        diff = _compute_function_difficulty(entry)
        assert diff.level == DifficultyLevel.IMPOSSIBLE
        assert diff.score == 1.0
        assert diff.oracle_tier_a is False

    def test_synthesizable_params(self):
        entry = {
            "status": "tier_b_hit",
            "target_module": "test",
            "target_function": "with_ctx",
            "tier_a_details": {
                "resolved_params": ["synthesizable:TxContext"],
                "has_object_params": False,
            },
            "tier_b_details": {"execution_success": True},
        }
        diff = _compute_function_difficulty(entry)
        assert "TxContext" in diff.requires_synthesis


class TestPackageOracle:
    """Tests for PackageOracle."""

    @pytest.fixture
    def sample_mm2_data(self):
        return {
            "accepted": [
                {
                    "status": "tier_b_hit",
                    "target_module": "cell",
                    "target_function": "new",
                    "target_package": "0xabc123",
                    "tier_a_details": {
                        "resolved_params": ["type_param[0]=U64"],
                        "has_object_params": False,
                    },
                    "tier_b_details": {"execution_success": True},
                },
                {
                    "status": "tier_a_hit",
                    "target_module": "cell",
                    "target_function": "get",
                    "target_package": "0xabc123",
                    "tier_a_details": {
                        "resolved_params": ["object"],
                        "has_object_params": True,
                    },
                },
                {
                    "status": "tier_a_hit",
                    "target_module": "cell",
                    "target_function": "set",
                    "target_package": "0xabc123",
                    "tier_a_details": {
                        "resolved_params": ["object", "U64"],
                        "has_object_params": True,
                    },
                },
            ],
            "rejected": [
                {
                    "status": "miss",
                    "target_module": "cell",
                    "target_function": "impossible",
                    "target_package": "0xabc123",
                    "failure_reason": "no constructor",
                    "failure_stage": "A3",
                },
            ],
        }

    def test_from_mm2_target_mapping(self, sample_mm2_data):
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            json.dump(sample_mm2_data, f)
            f.flush()

            oracle = PackageOracle.from_mm2_target_mapping(f.name)

        assert oracle.package_id == "0xabc123"
        assert oracle.total_functions == 4
        assert oracle.tier_a_possible == 3
        assert oracle.tier_a_impossible == 1
        assert oracle.tier_b_possible == 1
        assert oracle.tier_b_blocked_by_infra == 2
        assert oracle.max_synthesis_rate == 0.75
        assert oracle.max_execution_rate == 0.25

    def test_difficulty_distribution(self, sample_mm2_data):
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            json.dump(sample_mm2_data, f)
            f.flush()

            oracle = PackageOracle.from_mm2_target_mapping(f.name)

        # Should have at least one impossible function
        assert "impossible" in oracle.difficulty_distribution

    def test_to_dict(self, sample_mm2_data):
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            json.dump(sample_mm2_data, f)
            f.flush()

            oracle = PackageOracle.from_mm2_target_mapping(f.name)

        d = oracle.to_dict()
        assert "package_id" in d
        assert "total_functions" in d
        assert "max_synthesis_rate" in d
        assert "difficulty_distribution" in d

    def test_summary(self, sample_mm2_data):
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            json.dump(sample_mm2_data, f)
            f.flush()

            oracle = PackageOracle.from_mm2_target_mapping(f.name)

        summary = oracle.summary()
        assert "Package Oracle" in summary
        assert "Total functions: 4" in summary
        assert "Possible: 1" in summary  # tier_b

    def test_get_functions_by_difficulty(self, sample_mm2_data):
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            json.dump(sample_mm2_data, f)
            f.flush()

            oracle = PackageOracle.from_mm2_target_mapping(f.name)

        # Get all but impossible
        funcs = oracle.get_functions_by_difficulty(max_level=DifficultyLevel.VERY_DIFFICULT)
        assert all(f.level != DifficultyLevel.IMPOSSIBLE for f in funcs)

        # Get with limit
        funcs = oracle.get_functions_by_difficulty(limit=2)
        assert len(funcs) <= 2


class TestRankFunctionsForLlm:
    """Tests for rank_functions_for_llm."""

    @pytest.fixture
    def oracle_with_varied_difficulty(self):
        mm2_data = {
            "accepted": [
                {
                    "status": "tier_b_hit",
                    "target_module": "m",
                    "target_function": "easy",
                    "target_package": "0xabc",
                    "tier_a_details": {"resolved_params": [], "has_object_params": False},
                    "tier_b_details": {"execution_success": True},
                },
                {
                    "status": "tier_b_hit",
                    "target_module": "m",
                    "target_function": "medium",
                    "target_package": "0xabc",
                    "tier_a_details": {
                        "resolved_params": ["U64", "Bool", "construct:Type"],
                        "has_object_params": False,
                    },
                    "tier_b_details": {"execution_success": True},
                },
                {
                    "status": "tier_a_hit",
                    "target_module": "m",
                    "target_function": "hard",
                    "target_package": "0xabc",
                    "tier_a_details": {
                        "resolved_params": ["object", "type_param[0]=U64", "construct_hop:Config"],
                        "has_object_params": True,
                    },
                },
            ],
            "rejected": [
                {
                    "status": "miss",
                    "target_module": "m",
                    "target_function": "impossible",
                    "target_package": "0xabc",
                    "failure_reason": "no constructor",
                },
            ],
        }
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            json.dump(mm2_data, f)
            f.flush()
            return PackageOracle.from_mm2_target_mapping(f.name)

    def test_hardest_first(self, oracle_with_varied_difficulty):
        funcs = rank_functions_for_llm(
            oracle_with_varied_difficulty,
            strategy="hardest_first",
            exclude_impossible=True,
        )
        # Hard should come before easy
        func_names = [f.function for f in funcs]
        assert func_names.index("hard") < func_names.index("easy")

    def test_easiest_first(self, oracle_with_varied_difficulty):
        funcs = rank_functions_for_llm(
            oracle_with_varied_difficulty,
            strategy="easiest_first",
            exclude_impossible=True,
        )
        func_names = [f.function for f in funcs]
        assert func_names.index("easy") < func_names.index("hard")

    def test_executable_only(self, oracle_with_varied_difficulty):
        funcs = rank_functions_for_llm(
            oracle_with_varied_difficulty,
            strategy="executable_only",
        )
        # Only tier_b_hit functions
        assert all(f.oracle_tier_b for f in funcs)
        assert len(funcs) == 2  # easy and medium

    def test_exclude_impossible(self, oracle_with_varied_difficulty):
        # With exclude
        funcs = rank_functions_for_llm(
            oracle_with_varied_difficulty,
            exclude_impossible=True,
        )
        assert all(f.level != DifficultyLevel.IMPOSSIBLE for f in funcs)

        # Without exclude
        funcs = rank_functions_for_llm(
            oracle_with_varied_difficulty,
            exclude_impossible=False,
        )
        assert any(f.level == DifficultyLevel.IMPOSSIBLE for f in funcs)

    def test_limit(self, oracle_with_varied_difficulty):
        funcs = rank_functions_for_llm(
            oracle_with_varied_difficulty,
            limit=2,
            exclude_impossible=False,
        )
        assert len(funcs) == 2


class TestComputeOracleFromRunDir:
    """Tests for compute_oracle_from_run_dir."""

    def test_missing_mapping(self, tmp_path):
        oracle = compute_oracle_from_run_dir(tmp_path)
        assert oracle is None

    def test_with_mapping(self, tmp_path):
        mm2_data = {
            "accepted": [
                {
                    "status": "tier_b_hit",
                    "target_module": "test",
                    "target_function": "fn",
                    "target_package": "0xdef456",
                    "tier_a_details": {"resolved_params": [], "has_object_params": False},
                    "tier_b_details": {"execution_success": True},
                },
            ],
            "rejected": [],
        }
        (tmp_path / "mm2_target_mapping.json").write_text(json.dumps(mm2_data))

        oracle = compute_oracle_from_run_dir(tmp_path)
        assert oracle is not None
        assert oracle.total_functions == 1

    def test_with_run_config(self, tmp_path):
        mm2_data = {
            "accepted": [
                {
                    "status": "tier_b_hit",
                    "target_module": "test",
                    "target_function": "fn",
                    "target_package": "0xdef456",
                    "tier_a_details": {"resolved_params": [], "has_object_params": False},
                    "tier_b_details": {"execution_success": True},
                },
            ],
            "rejected": [],
        }
        (tmp_path / "mm2_target_mapping.json").write_text(json.dumps(mm2_data))
        (tmp_path / "run_config.json").write_text(json.dumps({"package_id": "0xcustom"}))

        oracle = compute_oracle_from_run_dir(tmp_path)
        assert oracle.package_id == "0xcustom"
