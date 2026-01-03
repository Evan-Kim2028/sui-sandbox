# Life of a Hit: A Step-by-Step Walkthrough

This guide traces the execution of a single benchmark package from extraction to a successful "Hit" (task completion). We will use the first package in our **Top-25** dataset as an example.

**Package ID:** `0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7`

---

## 1. Bytecode Extraction (The Ground Truth)
The process begins by running the Rust extractor on the package bytecode. 

**What happens:**
The extractor identifies all structs with the `key` ability. In this package, it finds one primary target:
- **Target:** `0xc681...::admin::AdminCap`

The extractor also maps out all public and private functions to find "Constructors" (functions that return this target type).

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
The harness invokes `smi_tx_sim` (Rust) to dry-run the generated plan on the Sui mainnet.

**The Result:**
The simulation returns "Transaction Success" and lists the objects created.
- **Created Object:** `0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7::admin::AdminCap`

---

## 6. Scoring (The Reward)
The `score.py` module compares the **Created Objects** against the **Target Set**.

- **Match Found:** The base types match exactly.
- **Score:** `1.0` (1 Hit / 1 Target).

---

## Summary of the "Reward"
For this package, the framework provided:
1. **Validation** that the model understands Move visibility.
2. **Quantification** of the model's ability to use "Progressive Exposure."
3. **Verification** that the resulting code actually executes on-chain.

This granular feedback allows researchers to move beyond "Pass/Fail" and understand the specific reasoning capabilities of their agents.
