"""Tests for PTB normalization and causality validation."""

from smi_bench.inhabit.normalize import CorrectionType, normalize_ptb_spec
from smi_bench.inhabit.validator import validate_ptb_causality_detailed


def test_normalize_object_to_imm_or_owned():
    """Test normalization of 'object' arg kind to 'imm_or_owned_object'."""
    ptb = {
        "calls": [
            {
                "target": "0x2::coin::transfer",
                "type_args": [],
                "args": [{"object": "0x123"}],
            }
        ]
    }
    result = normalize_ptb_spec(ptb)
    assert result.had_corrections
    assert result.spec["calls"][0]["args"][0] == {"imm_or_owned_object": "0x123"}
    assert CorrectionType.ARG_KIND_OBJECT_TO_IMM_OR_OWNED.value in result.histogram()


def test_normalize_object_id_to_imm_or_owned():
    """Test normalization of 'object_id' arg kind to 'imm_or_owned_object'."""
    ptb = {
        "calls": [
            {
                "target": "0x2::coin::transfer",
                "type_args": [],
                "args": [{"object_id": "0x456"}],
            }
        ]
    }
    result = normalize_ptb_spec(ptb)
    assert result.had_corrections
    assert result.spec["calls"][0]["args"][0] == {"imm_or_owned_object": "0x456"}
    assert CorrectionType.ARG_KIND_OBJECT_ID_TO_IMM_OR_OWNED.value in result.histogram()


def test_normalize_integer_strings():
    """Test normalization of string integers to int."""
    ptb = {
        "calls": [
            {
                "target": "0x2::coin::split",
                "type_args": [],
                "args": [{"imm_or_owned_object": "0x123"}, {"u64": "1000"}],
            }
        ]
    }
    result = normalize_ptb_spec(ptb)
    assert result.had_corrections
    assert result.spec["calls"][0]["args"][1] == {"u64": 1000}
    assert CorrectionType.INTEGER_STRING_TO_INT.value in result.histogram()


def test_normalize_result_ref_string():
    """Test normalization of string result references to int."""
    ptb = {
        "calls": [
            {"target": "0x2::coin::split", "type_args": [], "args": [{"imm_or_owned_object": "0x123"}, {"u64": 1000}]},
            {"target": "0x2::transfer::public_transfer", "type_args": [], "args": [{"result": "0"}]},
        ]
    }
    result = normalize_ptb_spec(ptb)
    assert result.had_corrections
    assert result.spec["calls"][1]["args"][0] == {"result": 0}
    assert CorrectionType.RESULT_REF_STRING_TO_INT.value in result.histogram()


def test_normalize_address_missing_0x():
    """Test adding 0x prefix to hex addresses."""
    ptb = {
        "calls": [
            {
                "target": "0x2::coin::transfer",
                "type_args": [],
                "args": [{"imm_or_owned_object": "123abc"}],
            }
        ]
    }
    result = normalize_ptb_spec(ptb)
    assert result.had_corrections
    assert result.spec["calls"][0]["args"][0] == {"imm_or_owned_object": "0x123abc"}
    assert CorrectionType.ADDRESS_MISSING_0X_PREFIX.value in result.histogram()


def test_normalize_multiple_corrections():
    """Test that multiple corrections are tracked."""
    ptb = {
        "calls": [
            {
                "target": "0x2::coin::split",
                "type_args": [],
                "args": [
                    {"object": "123"},  # object -> imm_or_owned_object + missing 0x
                    {"u64": "1000"},  # string int
                ],
            }
        ]
    }
    result = normalize_ptb_spec(ptb)
    assert result.had_corrections
    assert len(result.corrections) == 3  # object->imm_or_owned, missing 0x, string int
    hist = result.histogram()
    assert CorrectionType.ARG_KIND_OBJECT_TO_IMM_OR_OWNED.value in hist
    assert CorrectionType.ADDRESS_MISSING_0X_PREFIX.value in hist
    assert CorrectionType.INTEGER_STRING_TO_INT.value in hist


def test_normalize_no_corrections_needed():
    """Test that well-formed PTB specs pass through unchanged."""
    ptb = {
        "calls": [
            {
                "target": "0x2::coin::split",
                "type_args": [],
                "args": [{"imm_or_owned_object": "0x123"}, {"u64": 1000}],
            }
        ]
    }
    result = normalize_ptb_spec(ptb)
    assert not result.had_corrections
    assert result.spec == ptb


def test_causality_validation_valid():
    """Test causality validation for valid PTB."""
    ptb = {
        "calls": [
            {
                "target": "0x2::coin::split",
                "type_args": [],
                "args": [{"imm_or_owned_object": "0x123"}, {"u64": 1000}],
            },
            {
                "target": "0x2::transfer::public_transfer",
                "type_args": [],
                "args": [{"result": 0}, {"address": "0x456"}],
            },
        ]
    }
    result = validate_ptb_causality_detailed(ptb)
    assert result.valid
    assert len(result.errors) == 0
    assert result.call_count == 2
    assert result.result_references_total == 1
    assert result.result_references_valid == 1
    assert result.causality_score == 1.0


def test_causality_validation_future_reference():
    """Test causality validation catches future references."""
    ptb = {
        "calls": [
            {"target": "0x2::coin::split", "type_args": [], "args": [{"imm_or_owned_object": "0x123"}, {"result": 1}]},
            {"target": "0x2::transfer::public_transfer", "type_args": [], "args": [{"result": 0}]},
        ]
    }
    result = validate_ptb_causality_detailed(ptb)
    assert not result.valid
    assert len(result.errors) > 0
    assert "causality violation" in result.errors[0].lower()
    assert result.result_references_total == 2
    assert result.result_references_valid == 1
    assert result.causality_score == 0.5


def test_causality_validation_negative_index():
    """Test causality validation catches negative result indices."""
    ptb = {
        "calls": [
            {"target": "0x2::coin::split", "type_args": [], "args": [{"imm_or_owned_object": "0x123"}, {"u64": 1000}]},
            {"target": "0x2::transfer::public_transfer", "type_args": [], "args": [{"result": -1}]},
        ]
    }
    result = validate_ptb_causality_detailed(ptb)
    assert not result.valid
    assert len(result.errors) > 0
    assert "cannot be negative" in result.errors[0]


def test_causality_validation_no_references():
    """Test causality validation with no result references."""
    ptb = {
        "calls": [
            {"target": "0x2::coin::transfer", "type_args": [], "args": [{"imm_or_owned_object": "0x123"}]},
        ]
    }
    result = validate_ptb_causality_detailed(ptb)
    assert result.valid
    assert result.result_references_total == 0
    assert result.causality_score == 1.0  # No references means perfect causality


def test_normalize_shared_object_boolean():
    """Test normalization of boolean strings in shared_object."""
    ptb = {
        "calls": [
            {
                "target": "0x2::package::authorize_upgrade",
                "type_args": [],
                "args": [{"shared_object": {"id": "0x123", "mutable": "true"}}],
            }
        ]
    }
    result = normalize_ptb_spec(ptb)
    assert result.had_corrections
    assert result.spec["calls"][0]["args"][0]["shared_object"]["mutable"] is True
    assert CorrectionType.BOOLEAN_STRING_TO_BOOL.value in result.histogram()


def test_normalize_vector_integers():
    """Test normalization of string integers in vector args."""
    ptb = {
        "calls": [
            {
                "target": "0x2::vec_map::new",
                "type_args": [],
                "args": [{"vector_u64": ["1", "2", "3"]}],
            }
        ]
    }
    result = normalize_ptb_spec(ptb)
    assert result.had_corrections
    assert result.spec["calls"][0]["args"][0] == {"vector_u64": [1, 2, 3]}
    assert CorrectionType.INTEGER_STRING_TO_INT.value in result.histogram()
