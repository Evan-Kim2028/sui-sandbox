# Life of a Hit: A Step-by-Step Walkthrough

This guide traces the execution of a single benchmark package from extraction to a successful "Hit" (task completion). We will use the first package in our **Top-25** dataset as an example.

**Package ID:** `0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7`

---

## 1. Bytecode Extraction (The Ground Truth)
The process begins by running the Rust extractor on the package's compiled bytecode files.

**The Deserialization Process:**

1. **Load Binary Bytecode:**
   ```bash
   # The Rust CLI reads compiled .mv files
   ./target/release/sui_move_interface_extractor \
     --bytecode-package-dir ../sui-packages/packages/mainnet_most_used/0xc6/81...ade7 \
     --emit-bytecode-json -
   ```

2. **Parse Move VM Format:**
   - Uses `CompiledModule::deserialize_with_defaults()` to read binary Move VM bytecode
   - Extracts struct definitions from the type table
   - Extracts function signatures from the function table

3. **Build Canonical JSON:**
   - Normalizes addresses to `0x` + 64 hex chars
   - Converts Move abilities to canonical order: `["copy", "drop", "store", "key"]`
   - Maps Move types to JSON representation

**Concrete Example:**

**Input Directory Structure:**
```
0xc681.../bytecode_modules/
├── admin.mv              # Binary bytecode (not human-readable)
├── governance.mv
└── config.mv
```

**Rust Deserialization:**
```rust
// From src/bytecode.rs
let bytes = fs::read("bytecode_modules/admin.mv")?;
let module = CompiledModule::deserialize_with_defaults(&bytes)?;

// Extract struct with 'key' ability
for def in module.struct_defs() {
    let handle = module.datatype_handle_at(def.struct_handle);
    let abilities = ability_set_to_strings(&handle.abilities);
    
    // Found: AdminCap with ["key", "drop", "store"]
    if abilities.contains(&"key".to_string()) {
        // Mark as Phase II target
    }
}
```

**Output: Bytecode-Derived Interface JSON**
```json
{
  "schema_version": 1,
  "package_id": "0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7",
  "module_names": ["admin", "governance", "config"],
  "modules": {
    "admin": {
      "structs": {
        "AdminCap": {
          "abilities": ["key", "drop", "store"],
          "type_params": [],
          "is_native": false,
          "fields": [
            {"name": "id", "type": {"kind": "u64"}}
          ]
        }
      },
      "functions": {
        "create_admin_cap": {
          "visibility": "public",
          "is_entry": true,
          "params": [],
          "returns": [
            {
              "kind": "datatype",
              "address": "0xc681...ade7",
              "module": "admin",
              "name": "AdminCap",
              "type_args": []
            }
          ]
        }
      }
    }
  }
}
```

**What Gets Identified:**

From this interface, the benchmark identifies:

- **Phase I Discovery Target:**
  - Struct `AdminCap` has the `key` ability → This is a discoverable type
  - Function `create_admin_cap()` returns `AdminCap` → This is a constructor

- **Phase II Inhabitation Target:**
  - The model must generate a PTB that calls `create_admin_cap()`
  - The dry-run must produce an object of type `0xc681...ade7::admin::AdminCap`

The extractor also maps all other functions to identify alternative constructors and helper functions.

**Why Bytecode?**
This approach guarantees the ground truth represents exactly what the Move VM executes on-chain, independent of source code formatting or compilation artifacts.

---

## 2. Model Prompting (The Discovery)
The harness builds a prompt for the LLM (e.g., Gemini 3 Flash). It includes the package interface but **hides the abilities**. 

**The Challenge:**
The model must look at the function signatures and field types to infer that `AdminCap` is an important object and figure out how to create it.

---

## 3. Planning & Progressive Exposure
Models often realize they need more information.

**The "Need More" Request:**
The model returns:
```json
{
  "need_more": ["0xc681...::admin"],
  "reason": "I need to find the constructor for AdminCap in the admin module."
}
```

**The Response:**
The harness provides the full signatures for the `admin` module. The model identifies a function like `create_admin_cap()` and constructs a **PTB Plan**.

---

## 4. Normalization (The Fairness Layer)
The model might return slightly "sloppy" JSON:
```json
// Raw Model Output
{ "object": "0xc681..." } 
```

**The Fix:**
The `normalize.py` module automatically converts this to the strictly supported schema:
```json
// Normalized Output
{ "imm_or_owned_object": "0xc681..." }
```

---

## 5. Simulation (The Evidence)
The harness invokes `smi_tx_sim` (Rust) to evaluate the generated plan. There are two primary ways to run this:

### Option A: Static Analysis (`--simulation-mode build-only`)
The simulator uses **Move Model 2** to walk the call graph of the transaction.
- **Evidence:** The engine identifies that `create_admin_cap` is called and returns an `AdminCap`.
- **Pros:** No network required, no gas coins needed, works for "fresh" types not yet on mainnet.
- **Verification:** Correctly handles generics and prevents infinite loops with depth-limited traversal.

### Option B: On-Chain Dry-Run (`--simulation-mode dry-run`)
The simulator sends the transaction to a real Sui Fullnode for execution.
- **Evidence:** The RPC return "effects" show an object of type `AdminCap` was actually created in the Move VM.
- **Pros:** Maximum fidelity (verifies state, gas, and dynamic logic).
- **Requirement:** Requires a funded `SMI_SENDER` address on mainnet.

## 6. Scoring (The Reward)
The `score.py` module compares the **Created Objects** (from either static analysis or dry-run) against the **Target Set**.

- **Match Found:** The base types match exactly.
- **Score:** `1.0` (1 Hit / 1 Target).

---

## Summary of the "Reward"
For this package, the framework provided:
1. **Validation** that the model understands Move visibility.
2. **Quantification** of the model's ability to use "Progressive Exposure."
3. **Verification** that the resulting code actually executes on-chain.

This granular feedback allows researchers to move beyond "Pass/Fail" and understand the specific reasoning capabilities of their agents.
