# Research Insights & Framework Value

This framework is not just a benchmark; it is a specialized instrument for quantifying how well Large Language Models (LLMs) understand the semantic constraints of the Sui Move programming language.

## 1. The "Reward": Planning Intelligence

Unlike most coding benchmarks that focus on **Syntax Accuracy** (Did the model write valid JSON/Python?), this project focuses on **Planning Intelligence**.

### The Problem with Syntax Benchmarks
A model can produce 100% syntactically valid JSON PTB plans while having **zero** understanding of the Move logic. If it calls a "no-op" function instead of a constructor, it "passes" syntax but fails the task.

### How We Reward Intelligence
Our framework provides specific "Reward Signals" for researchers:
- **Autonomous Discovery**: We measure if the model can identify hidden constructors (private functions) that are invisible to standard RPC-based tools.
- **PTB Causality**: We verify if the model understands that Call B requires an object returned by Call A.
- **Resource Constraints**: We test if the model can adapt to limited on-chain inventory (Gas, Coins, AdminCaps).

---

## 2. Why Bytecode-First Matters

Using bytecode as the "Ground Truth" provides several high-value advantages for evaluation:

1. **Visibility of Private Logic**: Sui RPCs only expose `public` and `entry` functions. However, many important constructors are `private` or `friend`. Our Rust extractor finds these, allowing the benchmark to identify *why* a model failed (e.g., "The model couldn't find the constructor because it was private").
2. **Deterministic Abilities**: Bytecode stores struct `abilities` (key, store, copy, drop) explicitly. This allows us to mechanically label every struct in the ecosystem without manual tagging.
3. **Cross-Provider Stability**: Different RPC providers may normalize Move signatures differently. Bytecode extraction is provider-agnostic, ensuring your benchmark results are reproducible anywhere.

---

## 3. High-Signal Evaluation Loops

Researchers should focus on these high-signal metrics in the `evaluation_bundle`:

- **`planning_only_hit_rate`**: The most important metric. It factors out "noise" like JSON syntax errors and focuses purely on whether the LLM's logic was correct.
- **`causality_score`**: Measures the internal consistency of the transaction plan. A high score here indicates the model understands the data flow of the smart contract.
- **`interface_mode` Impact**: Tuning the `summarize_interface` mode (e.g., `entry_only` vs `entry_then_public`) allows you to measure how much "distraction" a model can handle.

---

## 4. Interpreting "Zeros"

In Phase II, a score of `0` is often more insightful than a `1`:
- **Execution OK / Hits 0**: The model found a valid execution path but missed the goal. This indicates a failure in **semantic intent**.
- **Execution Fail / Hits 0**: The model failed to understand Move preconditions (e.g., calling a function with the wrong arguments). This indicates a failure in **program logic**.

By analyzing these failure modes, you can determine if an agent needs better **tooling** (more context) or a better **foundation model** (stronger reasoning).

---

## 5. Common Architectural Failures

Observation of model performance across thousands of runs has revealed several recurring failure patterns:

### A. Result Reference Mismanagement
Models frequently attempt to use `{"input": 0}` or `{"arg": 0}` to refer to the output of a previous call. Our schema explicitly requires `{"result": index}`. High-performing models correctly grasp this "data-flow" dependency, whereas smaller models often hallucinate alternative keys.

### B. Arg Kind Confusion
A significant source of invalid PTBs is the use of `hex` or `object_id` as keys. While intuitive, these are not supported by the core simulator. The **Normalization** layer catches many of these, but models that produce the correct `imm_or_owned_object` kind upfront show a stronger alignment with the Move ecosystem's technical requirements.

### C. Recursive Depth Blindness
In many complex Sui packages, creating a target object requires a 2-step or 3-step sequence (e.g., `create_cap()` -> `mint_obj(cap)`). Models that fail to request additional module details via `need_more` often guess at 1-step execution paths, leading to `MoveAbort` errors. Successful agents demonstrate the ability to "unroll" these dependencies through progressive exploration.
