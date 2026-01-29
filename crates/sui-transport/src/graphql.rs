//! GraphQL Client for Sui Network
//!
//! Provides an alternative data fetching mechanism using Sui's GraphQL API.
//! Use as a fallback when JSON-RPC fails or doesn't provide sufficient data.
//!
//! ## Endpoints
//! - Mainnet: `https://graphql.mainnet.sui.io/graphql`
//! - Testnet: `https://graphql.testnet.sui.io/graphql`
//!
//! ## Pagination
//!
//! This module includes a generic [`Paginator`] that handles cursor-based pagination
//! automatically, following the Relay connection specification used by Sui's GraphQL API.
//!
//! ```ignore
//! // Automatic pagination - fetches multiple pages as needed
//! let digests = client.fetch_recent_transactions(200)?; // 4 pages of 50
//!
//! // Or use Paginator directly for custom queries
//! let paginator = Paginator::new(
//!     PaginationDirection::Forward,
//!     100,
//!     |cursor, page_size| my_custom_fetch(cursor, page_size),
//! );
//! let results = paginator.collect_all()?;
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! let client = GraphQLClient::mainnet();
//! let obj = client.fetch_object("0x...")?;
//! let pkg = client.fetch_package("0x2")?;
//! ```

use anyhow::{anyhow, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Maximum items per GraphQL page (Sui's server limit).
const MAX_PAGE_SIZE: usize = 50;

/// GraphQL client for Sui network queries.
#[derive(Clone)]
pub struct GraphQLClient {
    endpoint: String,
    agent: ureq::Agent,
}

/// Relay-style pagination info from GraphQL responses.
#[derive(Debug, Clone, Default)]
pub struct PageInfo {
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub start_cursor: Option<String>,
    pub end_cursor: Option<String>,
}

impl PageInfo {
    /// Parse PageInfo from a GraphQL response value.
    pub fn from_value(value: Option<&Value>) -> Self {
        let Some(v) = value else {
            return Self::default();
        };

        Self {
            has_next_page: v
                .get("hasNextPage")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            has_previous_page: v
                .get("hasPreviousPage")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            start_cursor: v
                .get("startCursor")
                .and_then(|x| x.as_str())
                .map(String::from),
            end_cursor: v
                .get("endCursor")
                .and_then(|x| x.as_str())
                .map(String::from),
        }
    }
}

/// Direction for cursor-based pagination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaginationDirection {
    /// Forward pagination using `first` and `after`.
    Forward,
    /// Backward pagination using `last` and `before`.
    Backward,
}

/// A generic paginator for GraphQL queries.
///
/// Handles cursor-based pagination automatically, supporting both forward
/// and backward iteration through results.
///
/// # Example
///
/// ```ignore
/// let paginator = Paginator::new(
///     PaginationDirection::Backward,
///     100, // total items wanted
///     |cursor, page_size| {
///         // Execute query and return (items, page_info)
///         client.fetch_page(cursor, page_size)
///     },
/// );
///
/// let all_items = paginator.collect_all()?;
/// ```
pub struct Paginator<T, F>
where
    F: FnMut(Option<&str>, usize) -> Result<(Vec<T>, PageInfo)>,
{
    direction: PaginationDirection,
    total_limit: usize,
    page_size: usize,
    fetch_fn: F,
    cursor: Option<String>,
    collected: usize,
    exhausted: bool,
    _marker: std::marker::PhantomData<T>,
}

impl<T, F> Paginator<T, F>
where
    F: FnMut(Option<&str>, usize) -> Result<(Vec<T>, PageInfo)>,
{
    /// Create a new paginator.
    ///
    /// - `direction`: Whether to paginate forward or backward
    /// - `total_limit`: Maximum total items to fetch across all pages
    /// - `fetch_fn`: Function that fetches a page given (cursor, page_size)
    pub fn new(direction: PaginationDirection, total_limit: usize, fetch_fn: F) -> Self {
        Self {
            direction,
            total_limit,
            page_size: MAX_PAGE_SIZE,
            fetch_fn,
            cursor: None,
            collected: 0,
            exhausted: false,
            _marker: std::marker::PhantomData,
        }
    }

    /// Set a custom page size (default is MAX_PAGE_SIZE).
    pub fn with_page_size(mut self, size: usize) -> Self {
        self.page_size = size.min(MAX_PAGE_SIZE);
        self
    }

    /// Fetch the next page of results.
    pub fn next_page(&mut self) -> Result<Option<Vec<T>>> {
        if self.exhausted || self.collected >= self.total_limit {
            return Ok(None);
        }

        let remaining = self.total_limit - self.collected;
        let page_size = remaining.min(self.page_size);

        let (items, page_info) = (self.fetch_fn)(self.cursor.as_deref(), page_size)?;

        if items.is_empty() {
            self.exhausted = true;
            return Ok(None);
        }

        self.collected += items.len();

        // Update cursor based on direction
        match self.direction {
            PaginationDirection::Forward => {
                if page_info.has_next_page {
                    self.cursor = page_info.end_cursor;
                } else {
                    self.exhausted = true;
                }
            }
            PaginationDirection::Backward => {
                if page_info.has_previous_page {
                    self.cursor = page_info.start_cursor;
                } else {
                    self.exhausted = true;
                }
            }
        }

        Ok(Some(items))
    }

    /// Collect all pages into a single vector.
    pub fn collect_all(mut self) -> Result<Vec<T>> {
        let mut all_items = Vec::with_capacity(self.total_limit);

        while let Some(page) = self.next_page()? {
            all_items.extend(page);
        }

        Ok(all_items)
    }
}

/// Object data returned from GraphQL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQLObject {
    pub address: String,
    pub version: u64,
    pub digest: Option<String>,
    pub type_string: Option<String>,
    pub owner: ObjectOwner,
    pub bcs_base64: Option<String>,
    pub content_json: Option<Value>,
}

/// Object ownership information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ObjectOwner {
    Address(String),
    Shared { initial_version: u64 },
    Immutable,
    Parent(String),
    Unknown,
}

/// Package data returned from GraphQL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQLPackage {
    pub address: String,
    pub version: u64,
    pub modules: Vec<GraphQLModule>,
}

/// Module data within a package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQLModule {
    pub name: String,
    pub bytecode_base64: Option<String>,
}

/// Dynamic field information returned from GraphQL.
///
/// Dynamic fields store child objects under a parent object using a key.
/// This struct captures both the key (name) and the stored value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicFieldInfo {
    /// Type of the key/name (e.g., "u64", "0x2::object::ID")
    pub name_type: String,
    /// BCS-encoded key bytes (base64)
    pub name_bcs: Option<String>,
    /// JSON representation of the key (for debugging)
    pub name_json: Option<Value>,
    /// Object ID of the dynamic field wrapper object (if it's a MoveObject)
    pub object_id: Option<String>,
    /// Version of the wrapper object
    pub version: Option<u64>,
    /// Digest of the wrapper object
    pub digest: Option<String>,
    /// Type of the stored value
    pub value_type: Option<String>,
    /// BCS-encoded value bytes (base64)
    pub value_bcs: Option<String>,
}

fn parse_dynamic_field_info(node: &Value) -> Option<DynamicFieldInfo> {
    let name = node.get("name")?;
    let value = node.get("value")?;

    // Parse the key/name
    let name_type = name
        .get("type")
        .and_then(|t| t.get("repr"))
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    let name_bcs = name
        .get("bcs")
        .and_then(|b| b.as_str())
        .map(|s| s.to_string());
    let name_json = name.get("json").cloned();

    // Parse the value (either MoveObject or MoveValue)
    let value_typename = value.get("__typename").and_then(|t| t.as_str());

    let (object_id, version, digest, value_type, value_bcs) = match value_typename {
        Some("MoveObject") => {
            let addr = value
                .get("address")
                .and_then(|a| a.as_str())
                .map(|s| s.to_string());
            let ver = value.get("version").and_then(|v| v.as_u64());
            let dig = value
                .get("digest")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string());
            let contents = value.get("contents");
            let vtype = contents
                .and_then(|c| c.get("type"))
                .and_then(|t| t.get("repr"))
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            let vbcs = contents
                .and_then(|c| c.get("bcs"))
                .and_then(|b| b.as_str())
                .map(|s| s.to_string());
            (addr, ver, dig, vtype, vbcs)
        }
        Some("MoveValue") => {
            let vtype = value
                .get("type")
                .and_then(|t| t.get("repr"))
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            let vbcs = value
                .get("bcs")
                .and_then(|b| b.as_str())
                .map(|s| s.to_string());
            (None, None, None, vtype, vbcs)
        }
        _ => (None, None, None, None, None),
    };

    Some(DynamicFieldInfo {
        name_type,
        name_bcs,
        name_json,
        object_id,
        version,
        digest,
        value_type,
        value_bcs,
    })
}

impl DynamicFieldInfo {
    /// Decode the BCS value bytes (from base64).
    pub fn decode_value_bcs(&self) -> Option<Vec<u8>> {
        use base64::Engine;
        self.value_bcs
            .as_ref()
            .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
    }

    /// Decode the BCS name/key bytes (from base64).
    pub fn decode_name_bcs(&self) -> Option<Vec<u8>> {
        use base64::Engine;
        self.name_bcs
            .as_ref()
            .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
    }
}

/// Full transaction block data with PTB details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQLTransaction {
    pub digest: String,
    pub sender: String,
    pub gas_budget: Option<u64>,
    pub gas_price: Option<u64>,
    pub timestamp_ms: Option<u64>,
    pub checkpoint: Option<u64>,
    /// PTB inputs
    pub inputs: Vec<GraphQLTransactionInput>,
    /// PTB commands
    pub commands: Vec<GraphQLCommand>,
    /// Effects
    pub effects: Option<GraphQLEffects>,
}

/// Transaction input (Pure, Object, SharedObject, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphQLTransactionInput {
    Pure {
        bytes_base64: String,
    },
    OwnedObject {
        address: String,
        version: u64,
        digest: String,
    },
    SharedObject {
        address: String,
        initial_shared_version: u64,
        mutable: bool,
    },
    Receiving {
        address: String,
        version: u64,
        digest: String,
    },
    // ImmutableObject is typically handled same as OwnedObject
}

/// PTB Command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphQLCommand {
    MoveCall {
        package: String,
        module: String,
        function: String,
        type_arguments: Vec<String>,
        arguments: Vec<GraphQLArgument>,
    },
    SplitCoins {
        coin: GraphQLArgument,
        amounts: Vec<GraphQLArgument>,
    },
    MergeCoins {
        destination: GraphQLArgument,
        sources: Vec<GraphQLArgument>,
    },
    TransferObjects {
        objects: Vec<GraphQLArgument>,
        address: GraphQLArgument,
    },
    MakeMoveVec {
        type_arg: Option<String>,
        elements: Vec<GraphQLArgument>,
    },
    Publish {
        modules: Vec<String>,
        dependencies: Vec<String>,
    },
    Upgrade {
        modules: Vec<String>,
        dependencies: Vec<String>,
        package: String,
        ticket: GraphQLArgument,
    },
    Other {
        typename: String,
    },
}

/// Command argument reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphQLArgument {
    Input(u16),
    Result(u16),
    NestedResult(u16, u16),
    GasCoin,
}

/// Transaction effects summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQLEffects {
    pub status: String,
    pub created: Vec<GraphQLObjectChange>,
    pub mutated: Vec<GraphQLObjectChange>,
    pub deleted: Vec<String>,
}

/// Object change in effects
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQLObjectChange {
    pub address: String,
    pub version: Option<u64>,
    pub digest: Option<String>,
}

impl GraphQLClient {
    /// Default request timeout in seconds (can be overridden by env).
    const DEFAULT_TIMEOUT_SECS: u64 = 30;
    /// Default connect timeout in seconds (can be overridden by env).
    const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

    fn default_timeouts() -> (Duration, Duration) {
        let timeout_secs = std::env::var("SUI_GRAPHQL_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(Self::DEFAULT_TIMEOUT_SECS);
        let connect_secs = std::env::var("SUI_GRAPHQL_CONNECT_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(Self::DEFAULT_CONNECT_TIMEOUT_SECS);
        (
            Duration::from_secs(timeout_secs),
            Duration::from_secs(connect_secs),
        )
    }

    fn build_agent(timeout: Duration, connect_timeout: Duration) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout(timeout)
            .timeout_connect(connect_timeout)
            .build()
    }

    /// Create a client for mainnet.
    pub fn mainnet() -> Self {
        Self::new("https://graphql.mainnet.sui.io/graphql")
    }

    /// Create a client for testnet.
    pub fn testnet() -> Self {
        Self::new("https://graphql.testnet.sui.io/graphql")
    }

    /// Create a client with a custom endpoint.
    pub fn new(endpoint: &str) -> Self {
        let (timeout, connect_timeout) = Self::default_timeouts();
        Self::with_timeouts(endpoint, timeout, connect_timeout)
    }

    /// Create a client with explicit timeouts.
    pub fn with_timeouts(endpoint: &str, timeout: Duration, connect_timeout: Duration) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            agent: Self::build_agent(timeout, connect_timeout),
        }
    }

    /// Execute a GraphQL query.
    fn query(&self, query: &str, variables: Option<Value>) -> Result<Value> {
        let body = serde_json::json!({
            "query": query,
            "variables": variables.unwrap_or(Value::Null)
        });

        let response: Value = self
            .agent
            .post(&self.endpoint)
            .set("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|e| anyhow!("GraphQL request failed: {}", e))?
            .into_json()
            .map_err(|e| anyhow!("Failed to parse GraphQL response: {}", e))?;

        // Check for GraphQL errors
        if let Some(errors) = response.get("errors") {
            if let Some(arr) = errors.as_array() {
                if !arr.is_empty() {
                    let msg = arr[0]
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown error");
                    return Err(anyhow!("GraphQL error: {}", msg));
                }
            }
        }

        response
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow!("No data in GraphQL response"))
    }

    /// Fetch an object by address.
    pub fn fetch_object(&self, address: &str) -> Result<GraphQLObject> {
        let query = r#"
            query GetObject($address: SuiAddress!) {
                object(address: $address) {
                    address
                    version
                    digest
                    owner {
                        __typename
                        ... on AddressOwner {
                            address { address }
                        }
                        ... on Shared {
                            initialSharedVersion
                        }
                        ... on ObjectOwner {
                            address { address }
                        }
                    }
                    asMoveObject {
                        contents {
                            type { repr }
                            bcs
                            json
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "address": address
        });

        let data = self.query(query, Some(variables))?;

        let obj = data
            .get("object")
            .ok_or_else(|| anyhow!("Object not found: {}", address))?;

        if obj.is_null() {
            return Err(anyhow!("Object not found: {}", address));
        }

        // Parse owner
        let owner = self.parse_owner(obj.get("owner"));

        // Parse MoveObject contents
        let move_obj = obj.get("asMoveObject").and_then(|m| m.get("contents"));
        let type_string = move_obj
            .and_then(|c| c.get("type"))
            .and_then(|t| t.get("repr"))
            .and_then(|r| r.as_str())
            .map(|s| s.to_string());
        let bcs_base64 = move_obj
            .and_then(|c| c.get("bcs"))
            .and_then(|b| b.as_str())
            .map(|s| s.to_string());
        let content_json = move_obj.and_then(|c| c.get("json")).cloned();

        Ok(GraphQLObject {
            address: obj
                .get("address")
                .and_then(|a| a.as_str())
                .unwrap_or(address)
                .to_string(),
            version: obj.get("version").and_then(|v| v.as_u64()).unwrap_or(1),
            digest: obj
                .get("digest")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string()),
            type_string,
            owner,
            bcs_base64,
            content_json,
        })
    }

    /// Fetch an object at a specific version.
    pub fn fetch_object_at_version(&self, address: &str, version: u64) -> Result<GraphQLObject> {
        let query = r#"
            query GetObjectAtVersion($address: SuiAddress!, $version: UInt53!) {
                object(address: $address, version: $version) {
                    address
                    version
                    digest
                    owner {
                        __typename
                        ... on AddressOwner {
                            address { address }
                        }
                        ... on Shared {
                            initialSharedVersion
                        }
                        ... on ObjectOwner {
                            address { address }
                        }
                    }
                    asMoveObject {
                        contents {
                            type { repr }
                            bcs
                            json
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "address": address,
            "version": version
        });

        let data = self.query(query, Some(variables))?;

        let obj = data
            .get("object")
            .ok_or_else(|| anyhow!("Object not found at version {}: {}", version, address))?;

        if obj.is_null() {
            return Err(anyhow!(
                "Object not found at version {}: {}",
                version,
                address
            ));
        }

        let owner = self.parse_owner(obj.get("owner"));
        let move_obj = obj.get("asMoveObject").and_then(|m| m.get("contents"));
        let type_string = move_obj
            .and_then(|c| c.get("type"))
            .and_then(|t| t.get("repr"))
            .and_then(|r| r.as_str())
            .map(|s| s.to_string());
        let bcs_base64 = move_obj
            .and_then(|c| c.get("bcs"))
            .and_then(|b| b.as_str())
            .map(|s| s.to_string());
        let content_json = move_obj.and_then(|c| c.get("json")).cloned();

        Ok(GraphQLObject {
            address: obj
                .get("address")
                .and_then(|a| a.as_str())
                .unwrap_or(address)
                .to_string(),
            version: obj
                .get("version")
                .and_then(|v| v.as_u64())
                .unwrap_or(version),
            digest: obj
                .get("digest")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string()),
            type_string,
            owner,
            bcs_base64,
            content_json,
        })
    }

    /// Fetch an object at a specific checkpoint.
    /// This is useful for historical replay when we know the checkpoint but not the exact version.
    pub fn fetch_object_at_checkpoint(
        &self,
        address: &str,
        checkpoint: u64,
    ) -> Result<GraphQLObject> {
        let snapshot_query = r#"
            query GetObjectAtCheckpoint($address: SuiAddress!, $checkpoint: UInt53!) {
                object(address: $address, version: null) @snapshot(at: $checkpoint) {
                    address
                    version
                    digest
                    owner {
                        __typename
                        ... on AddressOwner {
                            address { address }
                        }
                        ... on Shared {
                            initialSharedVersion
                        }
                        ... on ObjectOwner {
                            address { address }
                        }
                    }
                    asMoveObject {
                        contents {
                            type { repr }
                            bcs
                            json
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "address": address,
            "checkpoint": checkpoint
        });

        let data = self.query(snapshot_query, Some(variables))?;

        let obj = data
            .get("object")
            .ok_or_else(|| anyhow!("Object not found at checkpoint {}: {}", checkpoint, address))?;

        if obj.is_null() {
            return Err(anyhow!(
                "Object not found at checkpoint {}: {}",
                checkpoint,
                address
            ));
        }

        let owner = self.parse_owner(obj.get("owner"));
        let move_obj = obj.get("asMoveObject").and_then(|m| m.get("contents"));
        let type_string = move_obj
            .and_then(|c| c.get("type"))
            .and_then(|t| t.get("repr"))
            .and_then(|r| r.as_str())
            .map(|s| s.to_string());
        let bcs_base64 = move_obj
            .and_then(|c| c.get("bcs"))
            .and_then(|b| b.as_str())
            .map(|s| s.to_string());
        let content_json = move_obj.and_then(|c| c.get("json")).cloned();

        Ok(GraphQLObject {
            address: obj
                .get("address")
                .and_then(|a| a.as_str())
                .unwrap_or(address)
                .to_string(),
            version: obj.get("version").and_then(|v| v.as_u64()).unwrap_or(1),
            digest: obj
                .get("digest")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string()),
            type_string,
            owner,
            bcs_base64,
            content_json,
        })
    }

    /// Fetch a package with all its modules (handles pagination for large packages).
    pub fn fetch_package(&self, address: &str) -> Result<GraphQLPackage> {
        let mut all_modules: Vec<GraphQLModule> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pkg_address = address.to_string();
        let mut pkg_version = 1u64;

        // Paginate through all modules (GraphQL has 50 module limit per page)
        loop {
            let after_clause = cursor
                .as_ref()
                .map(|c| format!(", after: \"{}\"", c))
                .unwrap_or_default();

            let query = format!(
                r#"
                query GetPackage($address: SuiAddress!) {{
                    object(address: $address) {{
                        address
                        version
                        asMovePackage {{
                            modules(first: 50{}) {{
                                nodes {{
                                    name
                                    bytes
                                }}
                                pageInfo {{
                                    hasNextPage
                                    endCursor
                                }}
                            }}
                        }}
                    }}
                }}
                "#,
                after_clause
            );

            let variables = serde_json::json!({
                "address": address
            });

            let data = self.query(&query, Some(variables))?;

            let obj = data
                .get("object")
                .ok_or_else(|| anyhow!("Package not found: {}", address))?;

            if obj.is_null() {
                return Err(anyhow!("Package not found: {}", address));
            }

            // Get package info on first page
            if cursor.is_none() {
                pkg_address = obj
                    .get("address")
                    .and_then(|a| a.as_str())
                    .unwrap_or(address)
                    .to_string();
                pkg_version = obj.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
            }

            let pkg = obj
                .get("asMovePackage")
                .ok_or_else(|| anyhow!("Object is not a package: {}", address))?;

            let modules_data = pkg
                .get("modules")
                .ok_or_else(|| anyhow!("No modules field in package: {}", address))?;

            let modules_nodes = modules_data
                .get("nodes")
                .and_then(|n| n.as_array())
                .map(|arr| arr.to_vec())
                .unwrap_or_default();

            // Parse modules from this page
            for m in modules_nodes {
                all_modules.push(GraphQLModule {
                    name: m
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string(),
                    bytecode_base64: m
                        .get("bytes")
                        .and_then(|b| b.as_str())
                        .map(|s| s.to_string()),
                });
            }

            // Check for more pages
            let page_info = modules_data.get("pageInfo");
            let has_next = page_info
                .and_then(|p| p.get("hasNextPage"))
                .and_then(|h| h.as_bool())
                .unwrap_or(false);

            if !has_next {
                break;
            }

            cursor = page_info
                .and_then(|p| p.get("endCursor"))
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());
        }

        Ok(GraphQLPackage {
            address: pkg_address,
            version: pkg_version,
            modules: all_modules,
        })
    }

    /// Get the upgrade chain for a package, from current version to latest.
    ///
    /// Returns a list of (address, version) pairs representing all upgrades
    /// of this package. The first entry is the queried package, subsequent
    /// entries are upgrades in order.
    ///
    /// # Example
    /// ```ignore
    /// let upgrades = client.get_package_upgrades("0xb7c36a...")?;
    /// // Returns: [(v1, addr1), (v2, addr2), ..., (v6, latest_addr)]
    /// ```
    pub fn get_package_upgrades(&self, address: &str) -> Result<Vec<(String, u64)>> {
        let query = r#"
            query GetPackageUpgrades($address: SuiAddress!) {
                object(address: $address) {
                    address
                    version
                    asMovePackage {
                        packageVersionsAfter(first: 50) {
                            nodes {
                                address
                                version
                            }
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "address": address
        });

        let data = self.query(query, Some(variables))?;

        let obj = data
            .get("object")
            .ok_or_else(|| anyhow!("Package not found: {}", address))?;

        if obj.is_null() {
            return Err(anyhow!("Package not found: {}", address));
        }

        let mut result = Vec::new();

        // Add the queried package itself
        let pkg_addr = obj
            .get("address")
            .and_then(|a| a.as_str())
            .unwrap_or(address)
            .to_string();
        let pkg_version = obj.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
        result.push((pkg_addr, pkg_version));

        // Add upgrades
        if let Some(pkg) = obj.get("asMovePackage") {
            if let Some(versions) = pkg.get("packageVersionsAfter") {
                if let Some(nodes) = versions.get("nodes").and_then(|n| n.as_array()) {
                    for node in nodes {
                        let addr = node
                            .get("address")
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string();
                        let ver = node.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
                        if !addr.is_empty() && ver > 0 {
                            result.push((addr, ver));
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// Get the latest upgrade of a package (if any).
    ///
    /// Returns `Some((address, version))` for the latest upgrade,
    /// or `None` if the package has no upgrades (is at original version).
    pub fn get_latest_package_upgrade(&self, address: &str) -> Result<Option<(String, u64)>> {
        let query = r#"
            query GetLatestPackageUpgrade($address: SuiAddress!) {
                object(address: $address) {
                    address
                    version
                    asMovePackage {
                        packageVersionsAfter(last: 1) {
                            nodes {
                                address
                                version
                            }
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "address": address
        });

        let data = self.query(query, Some(variables))?;

        let obj = data
            .get("object")
            .ok_or_else(|| anyhow!("Package not found: {}", address))?;

        if obj.is_null() {
            return Err(anyhow!("Package not found: {}", address));
        }

        // Check for upgrades
        if let Some(pkg) = obj.get("asMovePackage") {
            if let Some(versions) = pkg.get("packageVersionsAfter") {
                if let Some(nodes) = versions.get("nodes").and_then(|n| n.as_array()) {
                    if let Some(node) = nodes.first() {
                        let addr = node
                            .get("address")
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string();
                        let ver = node.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
                        if !addr.is_empty() && ver > 0 {
                            return Ok(Some((addr, ver)));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    /// Fetch a package at a specific checkpoint (for historical replay).
    pub fn fetch_package_at_checkpoint(
        &self,
        address: &str,
        checkpoint: u64,
    ) -> Result<GraphQLPackage> {
        let query = r#"
            query GetPackageAtCheckpoint($address: SuiAddress!, $checkpoint: UInt53!) {
                checkpoint(sequenceNumber: $checkpoint) {
                    sequenceNumber
                }
                object(address: $address, version: null) @snapshot(at: $checkpoint) {
                    address
                    version
                    asMovePackage {
                        modules(first: 50) {
                            nodes {
                                name
                                bytes
                            }
                        }
                    }
                }
            }
        "#;

        // Try alternative query format if @snapshot doesn't work
        let alt_query = r#"
            query GetPackageAtCheckpoint($address: SuiAddress!) {
                object(address: $address) {
                    address
                    version
                    asMovePackage {
                        modules(first: 50) {
                            nodes {
                                name
                                bytes
                            }
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "address": address,
            "checkpoint": checkpoint
        });

        // Try with checkpoint first, fall back to current if not supported
        let data = match self.query(query, Some(variables.clone())) {
            Ok(d) => d,
            Err(_) => {
                // Fallback to simple query (no checkpoint support)
                let simple_vars = serde_json::json!({ "address": address });
                self.query(alt_query, Some(simple_vars))?
            }
        };

        let obj = data.get("object").ok_or_else(|| {
            anyhow!(
                "Package not found at checkpoint {}: {}",
                checkpoint,
                address
            )
        })?;

        if obj.is_null() {
            return Err(anyhow!(
                "Package not found at checkpoint {}: {}",
                checkpoint,
                address
            ));
        }

        let pkg = obj
            .get("asMovePackage")
            .ok_or_else(|| anyhow!("Object is not a package: {}", address))?;

        let modules_nodes = pkg
            .get("modules")
            .and_then(|m| m.get("nodes"))
            .and_then(|n| n.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default();

        let modules: Vec<GraphQLModule> = modules_nodes
            .iter()
            .map(|m| GraphQLModule {
                name: m
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string(),
                bytecode_base64: m
                    .get("bytes")
                    .and_then(|b| b.as_str())
                    .map(|s| s.to_string()),
            })
            .collect();

        Ok(GraphQLPackage {
            address: obj
                .get("address")
                .and_then(|a| a.as_str())
                .unwrap_or(address)
                .to_string(),
            version: obj.get("version").and_then(|v| v.as_u64()).unwrap_or(1),
            modules,
        })
    }

    /// Fetch a transaction by digest with full PTB details.
    /// Uses transactionJson for reliable access to type arguments.
    pub fn fetch_transaction(&self, digest: &str) -> Result<GraphQLTransaction> {
        // Use transactionJson for complete transaction data including typeArguments
        // The typed GraphQL commands query doesn't expose typeArguments properly
        let query = r#"
            query GetTransaction($digest: String!) {
                transaction(digest: $digest) {
                    digest
                    sender { address }
                    gasInput {
                        gasBudget
                        gasPrice
                    }
                    transactionJson
                    effects {
                        status
                        checkpoint { sequenceNumber }
                        timestamp
                        objectChanges {
                            nodes {
                                address
                                idCreated
                                idDeleted
                                outputState {
                                    version
                                    digest
                                }
                            }
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "digest": digest
        });

        let data = self.query(query, Some(variables))?;

        let tx = data
            .get("transaction")
            .ok_or_else(|| anyhow!("Transaction not found: {}", digest))?;

        if tx.is_null() {
            return Err(anyhow!("Transaction not found: {}", digest));
        }

        // Parse basic info
        let digest_str = tx
            .get("digest")
            .and_then(|d| d.as_str())
            .unwrap_or(digest)
            .to_string();

        let sender = tx
            .get("sender")
            .and_then(|s| s.get("address"))
            .and_then(|a| a.as_str())
            .unwrap_or("")
            .to_string();

        let gas_budget = tx
            .get("gasInput")
            .and_then(|g| g.get("gasBudget"))
            .and_then(|b| b.as_str())
            .and_then(|s| s.parse().ok());

        let gas_price = tx
            .get("gasInput")
            .and_then(|g| g.get("gasPrice"))
            .and_then(|p| p.as_str())
            .and_then(|s| s.parse().ok());

        // Parse inputs and commands from transactionJson (has complete type arguments)
        let (inputs, commands) = self.parse_transaction_json(tx)?;

        // Parse effects
        let effects = self.parse_transaction_effects(tx);

        let checkpoint = effects.as_ref().and_then(|_| {
            tx.get("effects")
                .and_then(|e| e.get("checkpoint"))
                .and_then(|c| c.get("sequenceNumber"))
                .and_then(|s| s.as_u64())
        });

        let timestamp_ms = tx
            .get("effects")
            .and_then(|e| e.get("timestamp"))
            .and_then(|t| t.as_str())
            .and_then(|s| {
                // Parse ISO timestamp to milliseconds
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.timestamp_millis() as u64)
            });

        Ok(GraphQLTransaction {
            digest: digest_str,
            sender,
            gas_budget,
            gas_price,
            timestamp_ms,
            checkpoint,
            inputs,
            commands,
            effects,
        })
    }

    /// Parse transaction from transactionJson field (has complete type arguments).
    fn parse_transaction_json(
        &self,
        tx: &Value,
    ) -> Result<(Vec<GraphQLTransactionInput>, Vec<GraphQLCommand>)> {
        // transactionJson can be either a string or a JSON object directly
        let owned_tx_json: Value;
        let tx_json: &Value = match tx.get("transactionJson") {
            Some(val) if val.is_string() => {
                let tx_json_str = val.as_str().unwrap();
                owned_tx_json = serde_json::from_str(tx_json_str)
                    .map_err(|e| anyhow!("Failed to parse transactionJson: {}", e))?;
                &owned_tx_json
            }
            Some(val) => val,
            None => return Err(anyhow!("Missing transactionJson field")),
        };

        let ptb = tx_json
            .get("kind")
            .and_then(|k| k.get("programmableTransaction"))
            .ok_or_else(|| anyhow!("Not a programmable transaction"))?;

        // Parse inputs
        let mut inputs = Vec::new();
        if let Some(input_nodes) = ptb.get("inputs").and_then(|i| i.as_array()) {
            for node in input_nodes {
                let kind = node.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                match kind {
                    "PURE" => {
                        let pure_bytes = node
                            .get("pure")
                            .and_then(|p| p.as_str())
                            .unwrap_or("")
                            .to_string();
                        inputs.push(GraphQLTransactionInput::Pure {
                            bytes_base64: pure_bytes,
                        });
                    }
                    "IMMUTABLE_OR_OWNED" => {
                        let address = node
                            .get("objectId")
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string();
                        let version = node
                            .get("version")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0);
                        let digest = node
                            .get("digest")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string();
                        inputs.push(GraphQLTransactionInput::OwnedObject {
                            address,
                            version,
                            digest,
                        });
                    }
                    "SHARED" => {
                        let address = node
                            .get("objectId")
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string();
                        let initial_shared_version = node
                            .get("version")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0);
                        let mutable = node
                            .get("mutable")
                            .and_then(|m| m.as_bool())
                            .unwrap_or(true);
                        inputs.push(GraphQLTransactionInput::SharedObject {
                            address,
                            initial_shared_version,
                            mutable,
                        });
                    }
                    _ => {}
                }
            }
        }

        // Parse commands
        let mut commands = Vec::new();
        if let Some(cmd_nodes) = ptb.get("commands").and_then(|c| c.as_array()) {
            for node in cmd_nodes {
                if let Some(mc) = node.get("moveCall") {
                    let package = mc
                        .get("package")
                        .and_then(|p| p.as_str())
                        .unwrap_or("")
                        .to_string();
                    let module = mc
                        .get("module")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string();
                    let function = mc
                        .get("function")
                        .and_then(|f| f.as_str())
                        .unwrap_or("")
                        .to_string();
                    let type_arguments: Vec<String> = mc
                        .get("typeArguments")
                        .and_then(|ta| ta.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let arguments: Vec<GraphQLArgument> = mc
                        .get("arguments")
                        .and_then(|a| a.as_array())
                        .map(|arr| arr.iter().map(|a| self.parse_json_argument(a)).collect())
                        .unwrap_or_default();

                    commands.push(GraphQLCommand::MoveCall {
                        package,
                        module,
                        function,
                        type_arguments,
                        arguments,
                    });
                } else if let Some(sc) = node.get("splitCoins") {
                    let coin = sc
                        .get("coin")
                        .map(|c| self.parse_json_argument(c))
                        .unwrap_or(GraphQLArgument::GasCoin);
                    let amounts: Vec<GraphQLArgument> = sc
                        .get("amounts")
                        .and_then(|a| a.as_array())
                        .map(|arr| arr.iter().map(|a| self.parse_json_argument(a)).collect())
                        .unwrap_or_default();
                    commands.push(GraphQLCommand::SplitCoins { coin, amounts });
                } else if let Some(mc) = node.get("mergeCoins") {
                    let destination = mc
                        .get("destination")
                        .map(|c| self.parse_json_argument(c))
                        .unwrap_or(GraphQLArgument::GasCoin);
                    let sources: Vec<GraphQLArgument> = mc
                        .get("sources")
                        .and_then(|s| s.as_array())
                        .map(|arr| arr.iter().map(|a| self.parse_json_argument(a)).collect())
                        .unwrap_or_default();
                    commands.push(GraphQLCommand::MergeCoins {
                        destination,
                        sources,
                    });
                } else if let Some(to) = node.get("transferObjects") {
                    let objects: Vec<GraphQLArgument> = to
                        .get("objects")
                        .and_then(|o| o.as_array())
                        .map(|arr| arr.iter().map(|a| self.parse_json_argument(a)).collect())
                        .unwrap_or_default();
                    let address = to
                        .get("address")
                        .map(|a| self.parse_json_argument(a))
                        .unwrap_or(GraphQLArgument::GasCoin);
                    commands.push(GraphQLCommand::TransferObjects { objects, address });
                } else if let Some(mv) = node.get("makeMoveVec") {
                    let type_arg = mv
                        .get("type")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string());
                    let elements: Vec<GraphQLArgument> = mv
                        .get("elements")
                        .and_then(|e| e.as_array())
                        .map(|arr| arr.iter().map(|a| self.parse_json_argument(a)).collect())
                        .unwrap_or_default();
                    commands.push(GraphQLCommand::MakeMoveVec { type_arg, elements });
                } else if let Some(pub_cmd) = node.get("publish") {
                    let modules: Vec<String> = pub_cmd
                        .get("modules")
                        .and_then(|m| m.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| m.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let dependencies: Vec<String> = pub_cmd
                        .get("dependencies")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|d| d.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    commands.push(GraphQLCommand::Publish {
                        modules,
                        dependencies,
                    });
                } else if let Some(up) = node.get("upgrade") {
                    let package = up
                        .get("package")
                        .and_then(|p| p.as_str())
                        .unwrap_or("")
                        .to_string();
                    let ticket = up
                        .get("ticket")
                        .map(|t| self.parse_json_argument(t))
                        .unwrap_or(GraphQLArgument::GasCoin);
                    let modules: Vec<String> = up
                        .get("modules")
                        .and_then(|m| m.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| m.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let dependencies: Vec<String> = up
                        .get("dependencies")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|d| d.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    commands.push(GraphQLCommand::Upgrade {
                        package,
                        ticket,
                        modules,
                        dependencies,
                    });
                }
            }
        }

        Ok((inputs, commands))
    }

    /// Parse argument from transactionJson format.
    fn parse_json_argument(&self, arg: &Value) -> GraphQLArgument {
        let kind = arg.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        match kind {
            "GAS_COIN" => GraphQLArgument::GasCoin,
            "INPUT" => {
                let index = arg.get("input").and_then(|i| i.as_u64()).unwrap_or(0) as u16;
                GraphQLArgument::Input(index)
            }
            "RESULT" => {
                let index = arg.get("cmd").and_then(|c| c.as_u64()).unwrap_or(0) as u16;
                GraphQLArgument::Result(index)
            }
            "NESTED_RESULT" => {
                let cmd = arg.get("cmd").and_then(|c| c.as_u64()).unwrap_or(0) as u16;
                let result_idx = arg.get("ix").and_then(|i| i.as_u64()).unwrap_or(0) as u16;
                GraphQLArgument::NestedResult(cmd, result_idx)
            }
            _ => GraphQLArgument::GasCoin,
        }
    }

    /// Parse transaction inputs from GraphQL response (legacy typed query).
    fn parse_transaction_inputs(&self, tx: &Value) -> Vec<GraphQLTransactionInput> {
        let mut inputs = Vec::new();

        let nodes = tx
            .get("kind")
            .and_then(|k| k.get("inputs"))
            .and_then(|i| i.get("nodes"))
            .and_then(|n| n.as_array());

        if let Some(nodes) = nodes {
            for node in nodes {
                let typename = node.get("__typename").and_then(|t| t.as_str());
                match typename {
                    Some("Pure") => {
                        let bytes = node
                            .get("bytes")
                            .and_then(|b| b.as_str())
                            .unwrap_or("")
                            .to_string();
                        inputs.push(GraphQLTransactionInput::Pure {
                            bytes_base64: bytes,
                        });
                    }
                    Some("OwnedOrImmutable") => {
                        if let Some(obj) = node.get("object") {
                            let address = obj
                                .get("address")
                                .and_then(|a| a.as_str())
                                .unwrap_or("")
                                .to_string();
                            let version = obj.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
                            let digest = obj
                                .get("digest")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string();
                            inputs.push(GraphQLTransactionInput::OwnedObject {
                                address,
                                version,
                                digest,
                            });
                        }
                    }
                    Some("SharedInput") => {
                        let address = node
                            .get("address")
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string();
                        let initial_shared_version = node
                            .get("initialSharedVersion")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let mutable = node
                            .get("mutable")
                            .and_then(|m| m.as_bool())
                            .unwrap_or(true);
                        inputs.push(GraphQLTransactionInput::SharedObject {
                            address,
                            initial_shared_version,
                            mutable,
                        });
                    }
                    Some("Receiving") => {
                        if let Some(obj) = node.get("object") {
                            let address = obj
                                .get("address")
                                .and_then(|a| a.as_str())
                                .unwrap_or("")
                                .to_string();
                            let version = obj.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
                            let digest = obj
                                .get("digest")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string();
                            inputs.push(GraphQLTransactionInput::Receiving {
                                address,
                                version,
                                digest,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        inputs
    }

    /// Parse a single argument from GraphQL response.
    fn parse_argument(&self, arg: &Value) -> GraphQLArgument {
        let typename = arg.get("__typename").and_then(|t| t.as_str());
        match typename {
            Some("Input") => {
                let idx = arg.get("ix").and_then(|i| i.as_u64()).unwrap_or(0) as u16;
                GraphQLArgument::Input(idx)
            }
            Some("TxResult") => {
                // TxResult has cmd (command index) and ix (result index within that command)
                let cmd_idx = arg.get("cmd").and_then(|i| i.as_u64()).unwrap_or(0) as u16;
                let res_idx = arg.get("ix").and_then(|i| i.as_u64()).unwrap_or(0) as u16;
                if res_idx == 0 {
                    GraphQLArgument::Result(cmd_idx)
                } else {
                    GraphQLArgument::NestedResult(cmd_idx, res_idx)
                }
            }
            Some("GasCoin") => GraphQLArgument::GasCoin,
            _ => GraphQLArgument::Input(0), // fallback
        }
    }

    /// Parse transaction commands from GraphQL response.
    fn parse_transaction_commands(&self, tx: &Value) -> Vec<GraphQLCommand> {
        let mut commands = Vec::new();

        let nodes = tx
            .get("kind")
            .and_then(|k| k.get("commands"))
            .and_then(|c| c.get("nodes"))
            .and_then(|n| n.as_array());

        if let Some(nodes) = nodes {
            for node in nodes {
                let typename = node
                    .get("__typename")
                    .and_then(|t| t.as_str())
                    .unwrap_or("");

                let cmd = match typename {
                    "MoveCallCommand" => {
                        let func = node.get("function");
                        let module_info = func.and_then(|f| f.get("module"));

                        let package = module_info
                            .and_then(|m| m.get("package"))
                            .and_then(|p| p.get("address"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string();

                        let module = module_info
                            .and_then(|m| m.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();

                        let function = func
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();

                        let type_arguments: Vec<String> = node
                            .get("typeArguments")
                            .and_then(|ta| ta.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|t| {
                                        // typeArguments is an array of objects with { repr: "..." }
                                        t.get("repr")
                                            .and_then(|r| r.as_str())
                                            .map(|s| s.to_string())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();

                        let arguments: Vec<GraphQLArgument> = node
                            .get("arguments")
                            .and_then(|a| a.as_array())
                            .map(|arr| arr.iter().map(|a| self.parse_argument(a)).collect())
                            .unwrap_or_default();

                        GraphQLCommand::MoveCall {
                            package,
                            module,
                            function,
                            type_arguments,
                            arguments,
                        }
                    }
                    "SplitCoinsCommand" => {
                        let coin = node
                            .get("coin")
                            .map(|c| self.parse_argument(c))
                            .unwrap_or(GraphQLArgument::GasCoin);
                        let amounts: Vec<GraphQLArgument> = node
                            .get("amounts")
                            .and_then(|a| a.as_array())
                            .map(|arr| arr.iter().map(|a| self.parse_argument(a)).collect())
                            .unwrap_or_default();

                        GraphQLCommand::SplitCoins { coin, amounts }
                    }
                    "MergeCoinsCommand" => {
                        let destination = node
                            .get("coin")
                            .map(|c| self.parse_argument(c))
                            .unwrap_or(GraphQLArgument::GasCoin);
                        let sources: Vec<GraphQLArgument> = node
                            .get("coins") // GraphQL uses "coins" not "sources"
                            .and_then(|s| s.as_array())
                            .map(|arr| arr.iter().map(|a| self.parse_argument(a)).collect())
                            .unwrap_or_default();

                        GraphQLCommand::MergeCoins {
                            destination,
                            sources,
                        }
                    }
                    "TransferObjectsCommand" => {
                        let objects: Vec<GraphQLArgument> = node
                            .get("inputs") // GraphQL uses "inputs" not "objects"
                            .and_then(|o| o.as_array())
                            .map(|arr| arr.iter().map(|a| self.parse_argument(a)).collect())
                            .unwrap_or_default();
                        let address = node
                            .get("address")
                            .map(|a| self.parse_argument(a))
                            .unwrap_or(GraphQLArgument::Input(0));

                        GraphQLCommand::TransferObjects { objects, address }
                    }
                    "MakeMoveVecCommand" => {
                        let type_arg = node
                            .get("type")
                            .and_then(|t| t.get("repr"))
                            .and_then(|r| r.as_str())
                            .map(|s| s.to_string());
                        let elements: Vec<GraphQLArgument> = node
                            .get("elements")
                            .and_then(|e| e.as_array())
                            .map(|arr| arr.iter().map(|a| self.parse_argument(a)).collect())
                            .unwrap_or_default();

                        GraphQLCommand::MakeMoveVec { type_arg, elements }
                    }
                    "PublishCommand" => {
                        let modules: Vec<String> = node
                            .get("modules")
                            .and_then(|m| m.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|m| m.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let dependencies: Vec<String> = node
                            .get("dependencies")
                            .and_then(|d| d.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|d| d.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();

                        GraphQLCommand::Publish {
                            modules,
                            dependencies,
                        }
                    }
                    "UpgradeCommand" => {
                        let modules: Vec<String> = node
                            .get("modules")
                            .and_then(|m| m.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|m| m.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let dependencies: Vec<String> = node
                            .get("dependencies")
                            .and_then(|d| d.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|d| d.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let package = node
                            .get("currentPackage")
                            .and_then(|p| p.as_str())
                            .unwrap_or("")
                            .to_string();
                        let ticket = node
                            .get("upgradeTicket")
                            .map(|t| self.parse_argument(t))
                            .unwrap_or(GraphQLArgument::Input(0));

                        GraphQLCommand::Upgrade {
                            modules,
                            dependencies,
                            package,
                            ticket,
                        }
                    }
                    _ => GraphQLCommand::Other {
                        typename: typename.to_string(),
                    },
                };

                commands.push(cmd);
            }
        }

        commands
    }

    /// Parse transaction effects from GraphQL response.
    fn parse_transaction_effects(&self, tx: &Value) -> Option<GraphQLEffects> {
        let effects = tx.get("effects")?;

        let status = effects
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("UNKNOWN")
            .to_string();

        let mut created = Vec::new();
        let mut mutated = Vec::new();
        let mut deleted = Vec::new();

        if let Some(changes) = effects
            .get("objectChanges")
            .and_then(|c| c.get("nodes"))
            .and_then(|n| n.as_array())
        {
            for change in changes {
                let addr = change
                    .get("address")
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_string();

                if addr.is_empty() {
                    continue;
                }

                let id_created = change
                    .get("idCreated")
                    .and_then(|c| c.as_bool())
                    .unwrap_or(false);
                let id_deleted = change
                    .get("idDeleted")
                    .and_then(|d| d.as_bool())
                    .unwrap_or(false);

                // Extract version and digest from outputState if available
                let output_state = change.get("outputState");
                let version = output_state
                    .and_then(|s| s.get("version"))
                    .and_then(|v| v.as_u64());
                let digest = output_state
                    .and_then(|s| s.get("digest"))
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string());

                if id_created && !id_deleted {
                    // Created
                    created.push(GraphQLObjectChange {
                        address: addr,
                        version,
                        digest,
                    });
                } else if id_deleted && !id_created {
                    // Deleted
                    deleted.push(addr);
                } else if !id_created && !id_deleted {
                    // Mutated (existed before, exists after)
                    mutated.push(GraphQLObjectChange {
                        address: addr,
                        version,
                        digest,
                    });
                }
            }
        }

        Some(GraphQLEffects {
            status,
            created,
            mutated,
            deleted,
        })
    }

    /// Fetch recent transactions from checkpoints.
    /// Returns just the digests - use `fetch_recent_transactions_full` for complete data.
    ///
    /// Uses automatic pagination to fetch more than MAX_PAGE_SIZE items.
    pub fn fetch_recent_transactions(&self, limit: usize) -> Result<Vec<String>> {
        let paginator =
            Paginator::new(PaginationDirection::Backward, limit, |cursor, page_size| {
                self.fetch_transaction_digests_page(cursor, page_size)
            });

        paginator.collect_all()
    }

    /// Fetch a single page of transaction digests (internal helper for pagination).
    fn fetch_transaction_digests_page(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<String>, PageInfo)> {
        let query = r#"
            query GetRecentTransactions($limit: Int!, $before: String) {
                transactions(last: $limit, before: $before) {
                    pageInfo {
                        hasNextPage
                        hasPreviousPage
                        startCursor
                        endCursor
                    }
                    nodes {
                        digest
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "limit": limit,
            "before": cursor
        });

        let data = self.query(query, Some(variables))?;

        let transactions = data.get("transactions");

        let digests = transactions
            .and_then(|t| t.get("nodes"))
            .and_then(|n| n.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|n| {
                        n.get("digest")
                            .and_then(|d| d.as_str())
                            .map(|s| s.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let page_info = PageInfo::from_value(transactions.and_then(|t| t.get("pageInfo")));

        Ok((digests, page_info))
    }

    /// Fetch recent transactions with full PTB data in a single query.
    /// This avoids consistency issues from fetching digests then individual transactions.
    ///
    /// Note: This includes ALL transaction types (including system transactions).
    /// Use `fetch_recent_ptb_transactions` to get only programmable transactions.
    pub fn fetch_recent_transactions_full(&self, limit: usize) -> Result<Vec<GraphQLTransaction>> {
        // For now, limit to 50 as the full transaction query is heavy
        let actual_limit = limit.min(50);

        let query = r#"
            query GetRecentTransactionsFull($limit: Int!) {
                transactions(last: $limit) {
                    nodes {
                        digest
                        sender { address }
                        expiration { epochId }
                        gasInput {
                            gasBudget
                            gasPrice
                        }
                        kind {
                            __typename
                            ... on ProgrammableTransaction {
                                inputs {
                                    nodes {
                                        __typename
                                        ... on Pure { bytes }
                                        ... on OwnedOrImmutable {
                                            object { address version digest }
                                        }
                                        ... on SharedInput {
                                            address
                                            initialSharedVersion
                                            mutable
                                        }
                                        ... on Receiving {
                                            object { address version digest }
                                        }
                                    }
                                }
                                commands {
                                    nodes {
                                        __typename
                                        ... on MoveCallCommand {
                                            function {
                                                module { name package { address } }
                                                name
                                            }
                                            arguments { __typename ... on Input { ix } ... on TxResult { cmd ix } ... on GasCoin { _ } }
                                        }
                                        ... on SplitCoinsCommand {
                                            coin { __typename ... on Input { ix } ... on TxResult { cmd ix } ... on GasCoin { _ } }
                                            amounts { __typename ... on Input { ix } ... on TxResult { cmd ix } ... on GasCoin { _ } }
                                        }
                                        ... on MergeCoinsCommand {
                                            coin { __typename ... on Input { ix } ... on TxResult { cmd ix } ... on GasCoin { _ } }
                                            coins { __typename ... on Input { ix } ... on TxResult { cmd ix } ... on GasCoin { _ } }
                                        }
                                        ... on TransferObjectsCommand {
                                            inputs { __typename ... on Input { ix } ... on TxResult { cmd ix } ... on GasCoin { _ } }
                                            address { __typename ... on Input { ix } ... on TxResult { cmd ix } ... on GasCoin { _ } }
                                        }
                                        ... on MakeMoveVecCommand {
                                            type { repr }
                                            elements { __typename ... on Input { ix } ... on TxResult { cmd ix } ... on GasCoin { _ } }
                                        }
                                        ... on PublishCommand {
                                            modules
                                            dependencies
                                        }
                                        ... on UpgradeCommand {
                                            modules
                                            dependencies
                                            currentPackage
                                            upgradeTicket { __typename ... on Input { ix } ... on TxResult { cmd ix } ... on GasCoin { _ } }
                                        }
                                    }
                                }
                            }
                        }
                        effects {
                            status
                            checkpoint { sequenceNumber }
                            timestamp
                            objectChanges {
                                nodes {
                                    address
                                    idCreated
                                    idDeleted
                                }
                            }
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "limit": actual_limit
        });

        let data = self.query(query, Some(variables))?;

        let nodes = data
            .get("transactions")
            .and_then(|t| t.get("nodes"))
            .and_then(|n| n.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default();

        let mut transactions = Vec::new();
        for tx in &nodes {
            let digest = tx
                .get("digest")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();

            let sender = tx
                .get("sender")
                .and_then(|s| s.get("address"))
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .to_string();

            let gas_budget = tx
                .get("gasInput")
                .and_then(|g| g.get("gasBudget"))
                .and_then(|b| b.as_str())
                .and_then(|s| s.parse().ok());

            let gas_price = tx
                .get("gasInput")
                .and_then(|g| g.get("gasPrice"))
                .and_then(|p| p.as_str())
                .and_then(|s| s.parse().ok());

            let inputs = self.parse_transaction_inputs(tx);
            let commands = self.parse_transaction_commands(tx);
            let effects = self.parse_transaction_effects(tx);

            let checkpoint = effects.as_ref().and_then(|_| {
                tx.get("effects")
                    .and_then(|e| e.get("checkpoint"))
                    .and_then(|c| c.get("sequenceNumber"))
                    .and_then(|s| s.as_u64())
            });

            let timestamp_ms = tx
                .get("effects")
                .and_then(|e| e.get("timestamp"))
                .and_then(|t| t.as_str())
                .and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .ok()
                        .map(|dt| dt.timestamp_millis() as u64)
                });

            transactions.push(GraphQLTransaction {
                digest,
                sender,
                gas_budget,
                gas_price,
                timestamp_ms,
                checkpoint,
                inputs,
                commands,
                effects,
            });
        }

        Ok(transactions)
    }

    /// Fetch recent programmable transactions only (filters out system transactions).
    /// System transactions have empty sender, zero gas budget, and no commands.
    ///
    /// This method fetches more transactions than requested and filters to get the
    /// requested number of PTB transactions.
    pub fn fetch_recent_ptb_transactions(&self, limit: usize) -> Result<Vec<GraphQLTransaction>> {
        // Fetch more to account for system transactions being filtered out
        // System transactions are roughly 30-40% of all transactions
        let fetch_limit = (limit * 2).min(50);

        let all_txs = self.fetch_recent_transactions_full(fetch_limit)?;

        let ptb_txs: Vec<GraphQLTransaction> = all_txs
            .into_iter()
            .filter(|tx| {
                // Filter out system transactions:
                // - They have empty sender
                // - They have zero gas budget
                // - They have no commands
                !tx.sender.is_empty() || tx.gas_budget.unwrap_or(0) > 0 || !tx.commands.is_empty()
            })
            .take(limit)
            .collect();

        Ok(ptb_txs)
    }

    /// Search for objects by type with automatic pagination.
    ///
    /// Uses forward pagination to fetch all matching objects up to the limit.
    pub fn search_objects_by_type(
        &self,
        type_filter: &str,
        limit: usize,
    ) -> Result<Vec<GraphQLObject>> {
        let type_filter = type_filter.to_string();

        let paginator = Paginator::new(PaginationDirection::Forward, limit, |cursor, page_size| {
            self.search_objects_page(&type_filter, cursor, page_size)
        });

        paginator.collect_all()
    }

    /// Fetch a single page of objects by type (internal helper for pagination).
    fn search_objects_page(
        &self,
        type_filter: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<GraphQLObject>, PageInfo)> {
        let query = r#"
            query SearchObjects($type: String!, $limit: Int!, $after: String) {
                objects(filter: { type: $type }, first: $limit, after: $after) {
                    pageInfo {
                        hasNextPage
                        hasPreviousPage
                        startCursor
                        endCursor
                    }
                    nodes {
                        address
                        version
                        digest
                        owner {
                            __typename
                            ... on AddressOwner {
                                address
                            }
                            ... on Shared {
                                initialSharedVersion
                            }
                            ... on Parent {
                                address
                            }
                        }
                        asMoveObject {
                            contents {
                                type { repr }
                                bcs
                            }
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "type": type_filter,
            "limit": limit,
            "after": cursor
        });

        let data = self.query(query, Some(variables))?;

        let objects_data = data.get("objects");

        let nodes = objects_data
            .and_then(|o| o.get("nodes"))
            .and_then(|n| n.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default();

        let objects: Vec<GraphQLObject> = nodes
            .iter()
            .filter_map(|obj| {
                let address = obj.get("address")?.as_str()?.to_string();
                let version = obj.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
                let digest = obj
                    .get("digest")
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string());

                let owner = self.parse_owner(obj.get("owner"));

                let move_obj = obj.get("asMoveObject").and_then(|m| m.get("contents"));
                let type_string = move_obj
                    .and_then(|c| c.get("type"))
                    .and_then(|t| t.get("repr"))
                    .and_then(|r| r.as_str())
                    .map(|s| s.to_string());
                let bcs_base64 = move_obj
                    .and_then(|c| c.get("bcs"))
                    .and_then(|b| b.as_str())
                    .map(|s| s.to_string());

                Some(GraphQLObject {
                    address,
                    version,
                    digest,
                    type_string,
                    owner,
                    bcs_base64,
                    content_json: None,
                })
            })
            .collect();

        let page_info = PageInfo::from_value(objects_data.and_then(|o| o.get("pageInfo")));

        Ok((objects, page_info))
    }

    /// Fetch dynamic fields (children) of an object.
    ///
    /// This is used to enumerate child objects stored via dynamic_field::add.
    /// Returns a list of (name, child_object) pairs where:
    /// - name is the BCS-encoded key
    /// - child_object contains the wrapped value
    ///
    /// For nested dynamic fields (like skip_list nodes), call this recursively.
    pub fn fetch_dynamic_fields(
        &self,
        parent_address: &str,
        limit: usize,
    ) -> Result<Vec<DynamicFieldInfo>> {
        let paginator = Paginator::new(PaginationDirection::Forward, limit, |cursor, page_size| {
            self.fetch_dynamic_fields_page(parent_address, cursor, page_size)
        });

        paginator.collect_all()
    }

    /// Fetch dynamic fields (children) of an object at a specific checkpoint.
    ///
    /// Falls back to current state if snapshot queries are not supported.
    pub fn fetch_dynamic_fields_at_checkpoint(
        &self,
        parent_address: &str,
        limit: usize,
        checkpoint: u64,
    ) -> Result<Vec<DynamicFieldInfo>> {
        let paginator = Paginator::new(PaginationDirection::Forward, limit, |cursor, page_size| {
            self.fetch_dynamic_fields_page_at_checkpoint(
                parent_address,
                cursor,
                page_size,
                checkpoint,
            )
        });

        paginator.collect_all()
    }

    /// Fetch a single dynamic field by name (type + BCS key).
    ///
    /// This is useful when the computed child ID doesn't match on-chain (e.g. upgrades),
    /// or when enumerating all fields is too expensive.
    pub fn fetch_dynamic_field_by_name(
        &self,
        parent_address: &str,
        name_type: &str,
        name_bcs: &[u8],
    ) -> Result<Option<DynamicFieldInfo>> {
        let name_bcs_b64 = base64::engine::general_purpose::STANDARD.encode(name_bcs);

        let query = r#"
            query GetDynamicFieldByName(
                $address: SuiAddress!,
                $nameType: String!,
                $nameBcs: Base64!
            ) {
                object(address: $address) {
                    dynamicField(name: { type: $nameType, bcs: $nameBcs }) {
                        name {
                            type { repr }
                            bcs
                            json
                        }
                        value {
                            __typename
                            ... on MoveObject {
                                address
                                version
                                digest
                                contents {
                                    type { repr }
                                    bcs
                                    json
                                }
                            }
                            ... on MoveValue {
                                type { repr }
                                bcs
                                json
                            }
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "address": parent_address,
            "nameType": name_type,
            "nameBcs": name_bcs_b64,
        });

        let data = self.query(query, Some(variables))?;
        let node = data
            .get("object")
            .and_then(|o| o.get("dynamicField"))
            .and_then(|df| if df.is_null() { None } else { Some(df) });

        Ok(node.and_then(parse_dynamic_field_info))
    }

    /// Fetch a single page of dynamic fields (internal helper).
    fn fetch_dynamic_fields_page(
        &self,
        parent_address: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<DynamicFieldInfo>, PageInfo)> {
        let query = r#"
            query GetDynamicFields($address: SuiAddress!, $limit: Int!, $after: String) {
                object(address: $address) {
                    dynamicFields(first: $limit, after: $after) {
                        pageInfo {
                            hasNextPage
                            hasPreviousPage
                            startCursor
                            endCursor
                        }
                        nodes {
                            name {
                                type { repr }
                                bcs
                                json
                            }
                            value {
                                __typename
                                ... on MoveObject {
                                    address
                                    version
                                    digest
                                    contents {
                                        type { repr }
                                        bcs
                                        json
                                    }
                                }
                                ... on MoveValue {
                                    type { repr }
                                    bcs
                                    json
                                }
                            }
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "address": parent_address,
            "limit": limit,
            "after": cursor
        });

        let data = self.query(query, Some(variables))?;

        let df_connection = data.get("object").and_then(|o| o.get("dynamicFields"));

        let nodes = df_connection
            .and_then(|df| df.get("nodes"))
            .and_then(|n| n.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default();

        let fields: Vec<DynamicFieldInfo> =
            nodes.iter().filter_map(parse_dynamic_field_info).collect();

        let page_info = PageInfo::from_value(df_connection.and_then(|df| df.get("pageInfo")));

        Ok((fields, page_info))
    }

    /// Fetch a single page of dynamic fields at a specific checkpoint (internal helper).
    fn fetch_dynamic_fields_page_at_checkpoint(
        &self,
        parent_address: &str,
        cursor: Option<&str>,
        limit: usize,
        checkpoint: u64,
    ) -> Result<(Vec<DynamicFieldInfo>, PageInfo)> {
        let query = r#"
            query GetDynamicFieldsAtCheckpoint(
                $address: SuiAddress!,
                $limit: Int!,
                $after: String,
                $checkpoint: UInt53!
            ) {
                object(address: $address, version: null) @snapshot(at: $checkpoint) {
                    dynamicFields(first: $limit, after: $after) {
                        pageInfo {
                            hasNextPage
                            hasPreviousPage
                            startCursor
                            endCursor
                        }
                        nodes {
                            name {
                                type { repr }
                                bcs
                                json
                            }
                            value {
                                __typename
                                ... on MoveObject {
                                    address
                                    version
                                    digest
                                    contents {
                                        type { repr }
                                        bcs
                                        json
                                    }
                                }
                                ... on MoveValue {
                                    type { repr }
                                    bcs
                                    json
                                }
                            }
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "address": parent_address,
            "limit": limit,
            "after": cursor,
            "checkpoint": checkpoint
        });

        let data = self.query(query, Some(variables))?;

        let df_connection = data.get("object").and_then(|o| o.get("dynamicFields"));

        let nodes = df_connection
            .and_then(|df| df.get("nodes"))
            .and_then(|n| n.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default();

        let fields: Vec<DynamicFieldInfo> =
            nodes.iter().filter_map(parse_dynamic_field_info).collect();

        let page_info = PageInfo::from_value(df_connection.and_then(|df| df.get("pageInfo")));

        Ok((fields, page_info))
    }

    /// Helper to parse owner from GraphQL response.
    fn parse_owner(&self, owner: Option<&Value>) -> ObjectOwner {
        let Some(owner) = owner else {
            return ObjectOwner::Unknown;
        };

        if owner.is_null() {
            return ObjectOwner::Immutable;
        }

        let typename = owner.get("__typename").and_then(|t| t.as_str());

        match typename {
            Some("AddressOwner") => {
                let addr = owner
                    .get("address")
                    .and_then(|a| a.get("address"))
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_string();
                ObjectOwner::Address(addr)
            }
            Some("Shared") => {
                let initial_version = owner
                    .get("initialSharedVersion")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                ObjectOwner::Shared { initial_version }
            }
            Some("Immutable") => ObjectOwner::Immutable,
            Some("ObjectOwner") => {
                // Object owned by another object (dynamic fields)
                let parent = owner
                    .get("address")
                    .and_then(|a| a.get("address"))
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_string();
                ObjectOwner::Parent(parent)
            }
            _ => ObjectOwner::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let mainnet = GraphQLClient::mainnet();
        assert!(mainnet.endpoint.contains("mainnet"));

        let testnet = GraphQLClient::testnet();
        assert!(testnet.endpoint.contains("testnet"));

        let custom = GraphQLClient::new("https://custom.endpoint");
        assert_eq!(custom.endpoint, "https://custom.endpoint");
    }

    /// Test fetching a well-known object (SUI framework package 0x2)
    /// Run with: cargo test test_fetch_framework_package -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_fetch_framework_package() {
        let client = GraphQLClient::mainnet();

        // Fetch the Sui framework package
        let result = client.fetch_package("0x2");
        assert!(
            result.is_ok(),
            "Failed to fetch package: {:?}",
            result.err()
        );

        let pkg = result.unwrap();
        // Address can be short or long form
        assert!(
            pkg.address.contains("0x2") || pkg.address.ends_with("02"),
            "Address should be 0x2, got: {}",
            pkg.address
        );
        assert!(!pkg.modules.is_empty(), "Package should have modules");

        // Print all modules found
        let module_names: Vec<&str> = pkg.modules.iter().map(|m| m.name.as_str()).collect();
        println!(
            "Fetched package 0x2 with {} modules: {:?}",
            pkg.modules.len(),
            module_names
        );

        // Should have coin module (this is a core module in sui framework)
        assert!(
            module_names.contains(&"coin"),
            "Should have coin module, got: {:?}",
            module_names
        );
    }

    /// Test fetching the clock object (shared, immutable address)
    /// Run with: cargo test test_fetch_clock_object -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_fetch_clock_object() {
        let client = GraphQLClient::mainnet();

        // The Clock object at 0x6
        let result = client.fetch_object("0x6");
        assert!(result.is_ok(), "Failed to fetch clock: {:?}", result.err());

        let obj = result.unwrap();
        assert!(
            obj.type_string
                .as_ref()
                .map(|t| t.contains("Clock"))
                .unwrap_or(false),
            "Object should be a Clock"
        );

        println!("Fetched clock object: {:?}", obj);
    }

    // Test the unified DataFetcher fallback behavior.
    // NOTE: This test uses DataFetcher from the main crate (sui-sandbox),
    // not from sui-transport. Run integration tests via the main crate instead.
    // Run with: cargo test test_data_fetcher_integration -- --ignored --nocapture
    // #[test]
    // #[ignore]
    // fn test_data_fetcher_integration() {
    //     use sui_sandbox::data_fetcher::DataFetcher;
    //
    //     let fetcher = DataFetcher::mainnet();
    //
    //     // Fetch an object using the unified fetcher
    //     let result = fetcher.fetch_object("0x6");
    //     assert!(result.is_ok(), "Failed to fetch object: {:?}", result.err());
    //
    //     let obj = result.unwrap();
    //     println!(
    //         "Fetched via DataFetcher: address={}, version={}, source={:?}",
    //         obj.address, obj.version, obj.source
    //     );
    // }

    /// Test fetching packages that were problematic in case studies
    /// Run with: cargo test test_problematic_packages -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_problematic_packages() {
        let client = GraphQLClient::mainnet();

        // Test 1: Artipedia upgraded package (caused LINKER_ERROR in case studies)
        println!("=== Testing Artipedia Upgraded Package ===");
        let artipedia_upgraded =
            "0x13fe3a7422946badff042be0e6dbbb0686fbff3fabc0c86cedc2d7a029486ece";
        match client.fetch_package(artipedia_upgraded) {
            Ok(pkg) => {
                println!(
                    "SUCCESS: Fetched {} with {} modules",
                    pkg.address,
                    pkg.modules.len()
                );
                for m in &pkg.modules {
                    println!(
                        "  - {} (bytecode: {} bytes)",
                        m.name,
                        m.bytecode_base64.as_ref().map(|b| b.len()).unwrap_or(0)
                    );
                }
            }
            Err(e) => println!("FAILED: {}", e),
        }

        // Test 2: Campaign package (caused MissingPackage error)
        println!("\n=== Testing Campaign Package ===");
        let campaign_pkg = "0x9f6de0f9c1333cecfafed4fd51ecf445d237a6295bd6ae88754821c8f8189789";
        match client.fetch_package(campaign_pkg) {
            Ok(pkg) => {
                println!(
                    "SUCCESS: Fetched {} with {} modules",
                    pkg.address,
                    pkg.modules.len()
                );
                for m in &pkg.modules {
                    println!(
                        "  - {} (bytecode: {} bytes)",
                        m.name,
                        m.bytecode_base64.as_ref().map(|b| b.len()).unwrap_or(0)
                    );
                }
            }
            Err(e) => println!("FAILED: {}", e),
        }

        // Test 3: Original artipedia embedded address
        println!("\n=== Testing Original Artipedia Address (embedded in bytecode) ===");
        let artipedia_original =
            "0xb7c36a747d6fdd6b59ab0354cea52a31df078c242242465a867481b6f4509498";
        match client.fetch_package(artipedia_original) {
            Ok(pkg) => {
                println!(
                    "SUCCESS: Fetched {} with {} modules",
                    pkg.address,
                    pkg.modules.len()
                );
                for m in &pkg.modules {
                    println!(
                        "  - {} (bytecode: {} bytes)",
                        m.name,
                        m.bytecode_base64.as_ref().map(|b| b.len()).unwrap_or(0)
                    );
                }
            }
            Err(e) => println!("FAILED: {}", e),
        }

        // Test 4: Try fetching as object instead of package
        println!("\n=== Testing Artipedia as Object (not package) ===");
        match client.fetch_object(artipedia_upgraded) {
            Ok(obj) => {
                println!(
                    "SUCCESS: address={}, version={}, type={:?}",
                    obj.address, obj.version, obj.type_string
                );
            }
            Err(e) => println!("FAILED: {}", e),
        }
    }

    /// Test fetching a full transaction with PTB details
    /// Run with: cargo test test_fetch_full_transaction -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_fetch_full_transaction() {
        let client = GraphQLClient::mainnet();

        // Test with Artipedia transaction (has MoveCall)
        println!("=== Testing Artipedia Transaction ===");
        let artipedia_digest = "AHKS3JQtTJC6Bwt7uE6v9z8kho2oQVHxCKvdsezJ9rHi";
        match client.fetch_transaction(artipedia_digest) {
            Ok(tx) => {
                println!("SUCCESS: Fetched transaction {}", tx.digest);
                println!("  Sender: {}", tx.sender);
                println!("  Gas budget: {:?}", tx.gas_budget);
                println!("  Gas price: {:?}", tx.gas_price);
                println!("  Timestamp: {:?}", tx.timestamp_ms);
                println!("  Checkpoint: {:?}", tx.checkpoint);
                println!("  Inputs ({}):", tx.inputs.len());
                for (i, input) in tx.inputs.iter().enumerate() {
                    println!("    [{}] {:?}", i, input);
                }
                println!("  Commands ({}):", tx.commands.len());
                for (i, cmd) in tx.commands.iter().enumerate() {
                    println!("    [{}] {:?}", i, cmd);
                }
                if let Some(effects) = &tx.effects {
                    println!("  Effects: status={}", effects.status);
                    println!("    Created: {}", effects.created.len());
                    println!("    Mutated: {}", effects.mutated.len());
                    println!("    Deleted: {}", effects.deleted.len());
                }
            }
            Err(e) => println!("FAILED: {}", e),
        }

        // Test with transfer transaction
        println!("\n=== Testing Transfer Transaction ===");
        let transfer_digest = "wSdGwVdYC1oJD4TcbJHenAgzD7gdwYRprSHE54pbMij";
        match client.fetch_transaction(transfer_digest) {
            Ok(tx) => {
                println!("SUCCESS: Fetched transaction {}", tx.digest);
                println!("  Inputs: {}", tx.inputs.len());
                println!("  Commands: {}", tx.commands.len());
                for (i, cmd) in tx.commands.iter().enumerate() {
                    println!("    [{}] {:?}", i, cmd);
                }
            }
            Err(e) => println!("FAILED: {}", e),
        }
    }

    /// Test fetching recent transactions
    /// Run with: cargo test test_fetch_recent_transactions -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_fetch_recent_transactions() {
        let client = GraphQLClient::mainnet();

        match client.fetch_recent_transactions(5) {
            Ok(digests) => {
                println!("SUCCESS: Fetched {} recent transactions", digests.len());
                for digest in &digests {
                    println!("  - {}", digest);
                }
            }
            Err(e) => println!("FAILED: {}", e),
        }
    }
}
