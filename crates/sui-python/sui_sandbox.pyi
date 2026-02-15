from typing import Any, Dict, List, Optional

__version__: str


class OrchestrationSession:
    def __init__(self) -> None: ...
    def prepare(
        self,
        package_id: str,
        *,
        resolve_deps: bool = ...,
        output_path: Optional[str] = ...,
    ) -> Dict[str, Any]: ...
    def load_context(self, context_path: str) -> Dict[str, Any]: ...
    def save_context(self, context_path: str) -> None: ...
    def has_context(self) -> bool: ...
    def package_id(self) -> Optional[str]: ...
    def context(self) -> Optional[Dict[str, Any]]: ...
    def replay(
        self,
        digest: Optional[str] = ...,
        *,
        checkpoint: Optional[int] = ...,
        discover_latest: Optional[int] = ...,
        source: Optional[str] = ...,
        state_file: Optional[str] = ...,
        cache_dir: Optional[str] = ...,
        walrus_network: str = ...,
        walrus_caching_url: Optional[str] = ...,
        walrus_aggregator_url: Optional[str] = ...,
        rpc_url: str = ...,
        profile: Optional[str] = ...,
        fetch_strategy: Optional[str] = ...,
        vm_only: bool = ...,
        allow_fallback: bool = ...,
        prefetch_depth: int = ...,
        prefetch_limit: int = ...,
        auto_system_objects: bool = ...,
        no_prefetch: bool = ...,
        compare: bool = ...,
        analyze_only: bool = ...,
        synthesize_missing: bool = ...,
        self_heal_dynamic_fields: bool = ...,
        analyze_mm2: bool = ...,
        verbose: bool = ...,
    ) -> Dict[str, Any]: ...


class FlowSession(OrchestrationSession): ...


class ContextSession(OrchestrationSession): ...


def extract_interface(
    *,
    package_id: Optional[str] = ...,
    bytecode_dir: Optional[str] = ...,
    rpc_url: str = ...,
) -> Dict[str, Any]: ...


def get_latest_checkpoint() -> int: ...


def get_checkpoint(checkpoint: int) -> Dict[str, Any]: ...


def doctor(
    *,
    rpc_url: str = ...,
    state_file: Optional[str] = ...,
    timeout_secs: int = ...,
    include_toolchain_checks: bool = ...,
) -> Dict[str, Any]: ...


def session_status(
    *,
    state_file: Optional[str] = ...,
    rpc_url: str = ...,
) -> Dict[str, Any]: ...


def session_reset(
    *,
    state_file: Optional[str] = ...,
) -> Dict[str, Any]: ...


def session_clean(
    *,
    state_file: Optional[str] = ...,
) -> Dict[str, Any]: ...


def snapshot_save(
    name: str,
    *,
    description: Optional[str] = ...,
    state_file: Optional[str] = ...,
) -> Dict[str, Any]: ...


def snapshot_load(
    name: str,
    *,
    state_file: Optional[str] = ...,
) -> Dict[str, Any]: ...


def snapshot_list() -> List[Dict[str, Any]]: ...


def snapshot_delete(name: str) -> Dict[str, Any]: ...


def ptb_universe(
    *,
    source: str = ...,
    latest: int = ...,
    top_packages: int = ...,
    max_ptbs: int = ...,
    out_dir: Optional[str] = ...,
    grpc_endpoint: Optional[str] = ...,
    stream_timeout_secs: int = ...,
) -> Dict[str, Any]: ...


def discover_checkpoint_targets(
    *,
    checkpoint: Optional[str] = ...,
    latest: Optional[int] = ...,
    package_id: Optional[str] = ...,
    include_framework: bool = ...,
    limit: int = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
) -> Dict[str, Any]: ...


def protocol_discover(
    *,
    protocol: str = ...,
    package_id: Optional[str] = ...,
    checkpoint: Optional[str] = ...,
    latest: Optional[int] = ...,
    include_framework: bool = ...,
    limit: int = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
) -> Dict[str, Any]: ...


def context_discover(
    *,
    checkpoint: Optional[str] = ...,
    latest: Optional[int] = ...,
    package_id: Optional[str] = ...,
    include_framework: bool = ...,
    limit: int = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
) -> Dict[str, Any]: ...


def adapter_discover(
    *,
    protocol: str = ...,
    package_id: Optional[str] = ...,
    checkpoint: Optional[str] = ...,
    latest: Optional[int] = ...,
    include_framework: bool = ...,
    limit: int = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
) -> Dict[str, Any]: ...


def workflow_validate(spec_path: str) -> Dict[str, Any]: ...


def pipeline_validate(spec_path: str) -> Dict[str, Any]: ...


def workflow_init(
    *,
    template: str = ...,
    output_path: Optional[str] = ...,
    format: Optional[str] = ...,
    digest: Optional[str] = ...,
    checkpoint: Optional[int] = ...,
    include_analyze_step: bool = ...,
    strict_replay: bool = ...,
    name: Optional[str] = ...,
    package_id: Optional[str] = ...,
    view_objects: List[str] = ...,
    force: bool = ...,
) -> Dict[str, Any]: ...


def pipeline_init(
    *,
    template: str = ...,
    output_path: Optional[str] = ...,
    format: Optional[str] = ...,
    digest: Optional[str] = ...,
    checkpoint: Optional[int] = ...,
    include_analyze_step: bool = ...,
    strict_replay: bool = ...,
    name: Optional[str] = ...,
    package_id: Optional[str] = ...,
    view_objects: List[str] = ...,
    force: bool = ...,
) -> Dict[str, Any]: ...


def workflow_auto(
    package_id: str,
    *,
    template: Optional[str] = ...,
    output_path: Optional[str] = ...,
    format: Optional[str] = ...,
    digest: Optional[str] = ...,
    discover_latest: Optional[int] = ...,
    checkpoint: Optional[int] = ...,
    name: Optional[str] = ...,
    best_effort: bool = ...,
    force: bool = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
) -> Dict[str, Any]: ...


def pipeline_auto(
    package_id: str,
    *,
    template: Optional[str] = ...,
    output_path: Optional[str] = ...,
    format: Optional[str] = ...,
    digest: Optional[str] = ...,
    discover_latest: Optional[int] = ...,
    checkpoint: Optional[int] = ...,
    name: Optional[str] = ...,
    best_effort: bool = ...,
    force: bool = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
) -> Dict[str, Any]: ...


def workflow_run(
    spec_path: str,
    *,
    dry_run: bool = ...,
    continue_on_error: bool = ...,
    report_path: Optional[str] = ...,
    rpc_url: str = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def pipeline_run(
    spec_path: str,
    *,
    dry_run: bool = ...,
    continue_on_error: bool = ...,
    report_path: Optional[str] = ...,
    rpc_url: str = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def workflow_run_inline(
    spec: Any,
    *,
    dry_run: bool = ...,
    continue_on_error: bool = ...,
    report_path: Optional[str] = ...,
    rpc_url: str = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def pipeline_run_inline(
    spec: Any,
    *,
    dry_run: bool = ...,
    continue_on_error: bool = ...,
    report_path: Optional[str] = ...,
    rpc_url: str = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def fetch_object_bcs(
    object_id: str,
    *,
    version: Optional[int] = ...,
    endpoint: Optional[str] = ...,
    api_key: Optional[str] = ...,
) -> Dict[str, Any]: ...


def import_state(
    *,
    state: Optional[str] = ...,
    transactions: Optional[str] = ...,
    objects: Optional[str] = ...,
    packages: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
) -> Dict[str, Any]: ...


def deserialize_transaction(raw_bcs: bytes) -> Dict[str, Any]: ...


def deserialize_package(bcs: bytes) -> Dict[str, Any]: ...


def fetch_package_bytecodes(
    package_id: str,
    *,
    resolve_deps: bool = ...,
) -> Dict[str, Any]: ...


def prepare_package_context(
    package_id: str,
    *,
    resolve_deps: bool = ...,
    output_path: Optional[str] = ...,
) -> Dict[str, Any]: ...


def context_prepare(
    package_id: str,
    *,
    resolve_deps: bool = ...,
    output_path: Optional[str] = ...,
) -> Dict[str, Any]: ...


def protocol_prepare(
    *,
    protocol: str = ...,
    package_id: Optional[str] = ...,
    resolve_deps: bool = ...,
    output_path: Optional[str] = ...,
) -> Dict[str, Any]: ...


def adapter_prepare(
    *,
    protocol: str = ...,
    package_id: Optional[str] = ...,
    resolve_deps: bool = ...,
    output_path: Optional[str] = ...,
) -> Dict[str, Any]: ...


def fetch_historical_package_bytecodes(
    package_ids: List[str],
    *,
    type_refs: List[str] = ...,
    checkpoint: Optional[int] = ...,
    endpoint: Optional[str] = ...,
    api_key: Optional[str] = ...,
) -> Dict[str, Any]: ...


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
    historical_versions: Optional[Dict[str, int]] = ...,
    fetch_child_objects: bool = ...,
    grpc_endpoint: Optional[str] = ...,
    grpc_api_key: Optional[str] = ...,
    package_bytecodes: Optional[Dict[str, Any]] = ...,
    fetch_deps: bool = ...,
) -> Dict[str, Any]: ...


def historical_view_from_versions(
    *,
    versions_file: str,
    package_id: str,
    module: str,
    function: str,
    required_objects: List[str],
    type_args: List[str] = ...,
    package_roots: List[str] = ...,
    type_refs: List[str] = ...,
    fetch_child_objects: bool = ...,
    grpc_endpoint: Optional[str] = ...,
    grpc_api_key: Optional[str] = ...,
) -> Dict[str, Any]: ...


def historical_decode_return_u64(
    result: Any,
    *,
    command_index: int = ...,
    value_index: int,
) -> Optional[int]: ...


def historical_decode_return_u64s(
    result: Any,
    *,
    command_index: int = ...,
) -> Optional[List[Optional[int]]]: ...


def historical_decode_returns_typed(
    result: Any,
    *,
    command_index: int = ...,
) -> Optional[List[Dict[str, Any]]]: ...


def historical_decode_with_schema(
    result: Any,
    schema: List[Dict[str, Any]],
    *,
    command_index: int = ...,
) -> Optional[Dict[str, Any]]: ...


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
    context_path: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    profile: Optional[str] = ...,
    fetch_strategy: Optional[str] = ...,
    vm_only: bool = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    no_prefetch: bool = ...,
    compare: bool = ...,
    analyze_only: bool = ...,
    synthesize_missing: bool = ...,
    self_heal_dynamic_fields: bool = ...,
    analyze_mm2: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def replay_transaction(
    digest: Optional[str] = ...,
    *,
    checkpoint: Optional[int] = ...,
    discover_latest: Optional[int] = ...,
    discover_package_id: Optional[str] = ...,
    source: Optional[str] = ...,
    state_file: Optional[str] = ...,
    context_path: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    rpc_url: str = ...,
    profile: Optional[str] = ...,
    fetch_strategy: Optional[str] = ...,
    vm_only: bool = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    no_prefetch: bool = ...,
    compare: bool = ...,
    analyze_only: bool = ...,
    synthesize_missing: bool = ...,
    self_heal_dynamic_fields: bool = ...,
    analyze_mm2: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def analyze_replay(
    digest: Optional[str] = ...,
    *,
    checkpoint: Optional[int] = ...,
    discover_latest: Optional[int] = ...,
    discover_package_id: Optional[str] = ...,
    source: Optional[str] = ...,
    state_file: Optional[str] = ...,
    context_path: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    rpc_url: str = ...,
    profile: Optional[str] = ...,
    fetch_strategy: Optional[str] = ...,
    vm_only: bool = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    no_prefetch: bool = ...,
    analyze_mm2: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def replay_analyze(
    digest: Optional[str] = ...,
    *,
    checkpoint: Optional[int] = ...,
    discover_latest: Optional[int] = ...,
    discover_package_id: Optional[str] = ...,
    source: Optional[str] = ...,
    state_file: Optional[str] = ...,
    context_path: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    rpc_url: str = ...,
    profile: Optional[str] = ...,
    fetch_strategy: Optional[str] = ...,
    vm_only: bool = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    no_prefetch: bool = ...,
    analyze_mm2: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def replay_effects(
    digest: Optional[str] = ...,
    *,
    checkpoint: Optional[int] = ...,
    discover_latest: Optional[int] = ...,
    discover_package_id: Optional[str] = ...,
    source: Optional[str] = ...,
    state_file: Optional[str] = ...,
    context_path: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    rpc_url: str = ...,
    profile: Optional[str] = ...,
    fetch_strategy: Optional[str] = ...,
    vm_only: bool = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    no_prefetch: bool = ...,
    compare: bool = ...,
    synthesize_missing: bool = ...,
    self_heal_dynamic_fields: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def classify_replay_result(result: Any) -> Dict[str, Any]: ...


def dynamic_field_diagnostics(
    digest: Optional[str] = ...,
    *,
    checkpoint: Optional[int] = ...,
    discover_latest: Optional[int] = ...,
    discover_package_id: Optional[str] = ...,
    source: Optional[str] = ...,
    state_file: Optional[str] = ...,
    context_path: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    rpc_url: str = ...,
    profile: Optional[str] = ...,
    fetch_strategy: Optional[str] = ...,
    vm_only: bool = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    analyze_mm2: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def context_replay(
    digest: Optional[str] = ...,
    *,
    checkpoint: Optional[int] = ...,
    discover_latest: Optional[int] = ...,
    discover_package_id: Optional[str] = ...,
    source: Optional[str] = ...,
    state_file: Optional[str] = ...,
    context_path: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    rpc_url: str = ...,
    profile: Optional[str] = ...,
    fetch_strategy: Optional[str] = ...,
    vm_only: bool = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    no_prefetch: bool = ...,
    compare: bool = ...,
    analyze_only: bool = ...,
    synthesize_missing: bool = ...,
    self_heal_dynamic_fields: bool = ...,
    analyze_mm2: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def protocol_run(
    digest: Optional[str] = ...,
    *,
    protocol: str = ...,
    package_id: Optional[str] = ...,
    resolve_deps: bool = ...,
    context_path: Optional[str] = ...,
    checkpoint: Optional[int] = ...,
    discover_latest: Optional[int] = ...,
    source: Optional[str] = ...,
    state_file: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    rpc_url: str = ...,
    profile: Optional[str] = ...,
    fetch_strategy: Optional[str] = ...,
    vm_only: bool = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    no_prefetch: bool = ...,
    compare: bool = ...,
    analyze_only: bool = ...,
    synthesize_missing: bool = ...,
    self_heal_dynamic_fields: bool = ...,
    analyze_mm2: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def adapter_run(
    digest: Optional[str] = ...,
    *,
    protocol: str = ...,
    package_id: Optional[str] = ...,
    resolve_deps: bool = ...,
    context_path: Optional[str] = ...,
    checkpoint: Optional[int] = ...,
    discover_latest: Optional[int] = ...,
    source: Optional[str] = ...,
    state_file: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    rpc_url: str = ...,
    profile: Optional[str] = ...,
    fetch_strategy: Optional[str] = ...,
    vm_only: bool = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    no_prefetch: bool = ...,
    compare: bool = ...,
    analyze_only: bool = ...,
    synthesize_missing: bool = ...,
    self_heal_dynamic_fields: bool = ...,
    analyze_mm2: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...


def context_run(
    package_id: str,
    digest: Optional[str] = ...,
    *,
    resolve_deps: bool = ...,
    context_path: Optional[str] = ...,
    checkpoint: Optional[int] = ...,
    discover_latest: Optional[int] = ...,
    source: Optional[str] = ...,
    state_file: Optional[str] = ...,
    cache_dir: Optional[str] = ...,
    walrus_network: str = ...,
    walrus_caching_url: Optional[str] = ...,
    walrus_aggregator_url: Optional[str] = ...,
    rpc_url: str = ...,
    profile: Optional[str] = ...,
    fetch_strategy: Optional[str] = ...,
    vm_only: bool = ...,
    allow_fallback: bool = ...,
    prefetch_depth: int = ...,
    prefetch_limit: int = ...,
    auto_system_objects: bool = ...,
    no_prefetch: bool = ...,
    compare: bool = ...,
    analyze_only: bool = ...,
    synthesize_missing: bool = ...,
    self_heal_dynamic_fields: bool = ...,
    analyze_mm2: bool = ...,
    verbose: bool = ...,
) -> Dict[str, Any]: ...
