from typing import Any, Dict, List, Optional

__version__: str


def extract_interface(*, package_id: Optional[str] = ..., bytecode_dir: Optional[str] = ..., rpc_url: str = ...) -> Dict[str, Any]: ...

def get_latest_checkpoint() -> int: ...

def get_checkpoint(checkpoint: int) -> Dict[str, Any]: ...

def import_state(*, state: Optional[str] = ..., transactions: Optional[str] = ..., objects: Optional[str] = ..., packages: Optional[str] = ..., cache_dir: Optional[str] = ...) -> Dict[str, Any]: ...

def deserialize_transaction(raw_bcs: bytes) -> Dict[str, Any]: ...

def deserialize_package(bcs: bytes) -> Dict[str, Any]: ...

def fetch_package_bytecodes(package_id: str, *, resolve_deps: bool = ...) -> Dict[str, Any]: ...

def json_to_bcs(type_str: str, object_json: str, package_bytecodes: List[bytes]) -> bytes: ...

def transaction_json_to_bcs(transaction_json: str) -> bytes: ...

def call_view_function(
    package_id: str,
    module: str,
    function: str,
    *,
    type_args: List[str] = ...,
    object_inputs: List[Dict[str, Any]] = ...,
    pure_inputs: List[bytes] = ...,
    child_objects: Optional[Dict[str, List[Dict[str, Any]]]] = ...,
    package_bytecodes: Optional[Dict[str, List[bytes]]] = ...,
    fetch_deps: bool = ...,
) -> Dict[str, Any]: ...

def fuzz_function(
    package_id: str,
    module: str,
    function: str,
    *,
    iterations: int = ...,
    seed: Optional[int] = ...,
    sender: str = ...,
    gas_budget: int = ...,
    type_args: List[str] = ...,
    fail_fast: bool = ...,
    max_vector_len: int = ...,
    dry_run: bool = ...,
    fetch_deps: bool = ...,
) -> Dict[str, Any]: ...

def replay(
    digest: Optional[str] = ...,
    *,
    rpc_url: str = ...,
    source: str = ...,
    checkpoint: Optional[int] = ...,
    state_file: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    no_prefetch: bool = ...,
    compare: bool = ...,
    analyze_only: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...
