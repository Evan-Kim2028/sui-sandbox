"""Quick smoke test for the sui_sandbox Python bindings."""

import sui_sandbox

print("=" * 60)
print("sui_sandbox Python Bindings Test")
print("=" * 60)

# 1. Get latest checkpoint from Walrus (no auth needed)
print("\n[1] Fetching latest Walrus checkpoint...")
latest = sui_sandbox.get_latest_checkpoint()
print(f"    Latest checkpoint: {latest:,}")

# 2. Fetch that checkpoint summary
print(f"\n[2] Fetching checkpoint {latest:,} from Walrus...")
cp_data = sui_sandbox.get_checkpoint(latest)
print(f"    Epoch:             {cp_data['epoch']}")
print(f"    Timestamp (ms):    {cp_data['timestamp_ms']}")
print(f"    Transactions:      {cp_data['transaction_count']}")
print(f"    Object versions:   {cp_data['object_versions_count']}")

# 3. Show a few transactions from that checkpoint
if cp_data["transactions"]:
    print(f"\n[3] Sample transactions from checkpoint {latest:,}:")
    for i, tx in enumerate(cp_data["transactions"][:5]):
        print(f"    [{i}] digest={tx['digest'][:16]}...  sender={tx['sender'][:16]}...  "
              f"cmds={tx['commands']}  in={tx['input_objects']}  out={tx['output_objects']}")

    # 4. Walrus-only analyze replay on the first transaction
    first_tx = cp_data["transactions"][0]
    digest = first_tx["digest"]
    print(f"\n[4] Walrus analyze_replay for {digest[:24]}...")
    replay_info = sui_sandbox.replay(
        digest=digest,
        checkpoint=latest,
        analyze_only=True,
        verbose=False,
    )
    print(f"    Sender:    {replay_info['sender']}")
    print(f"    Commands:  {replay_info['commands']}")
    print(f"    Inputs:    {replay_info['inputs']}")
    print(f"    Objects:   {replay_info['objects']}")
    print(f"    Packages:  {replay_info['packages']}")
    print(f"    Modules:   {replay_info['modules']}")
    print(f"    Epoch:     {replay_info['epoch']}")
    print(f"    Protocol:  {replay_info['protocol_version']}")

    # Show command breakdown
    print(f"\n    Command breakdown:")
    for cmd in replay_info.get("command_summaries", []):
        target = cmd.get("target", "")
        line = f"      {cmd['kind']}"
        if target:
            line += f"  -> {target}"
        line += f"  (type_args={cmd['type_args']}, args={cmd['args']})"
        print(line)

    # Input summary
    inp = replay_info.get("input_summary", {})
    print(f"\n    Input summary:")
    for k, v in inp.items():
        if k != "total":
            print(f"      {k}: {v}")
else:
    print("\n    (No transactions in this checkpoint)")

# 5. Extract the Sui framework package interface (0x2) via GraphQL
print(f"\n[5] Extracting package 0x2 interface (Sui framework)...")
iface = sui_sandbox.extract_interface(package_id="0x2")
modules = iface.get("modules", {})
module_names = sorted(modules.keys())
fn_count = sum(len(m.get("functions", {})) for m in modules.values())
struct_count = sum(len(m.get("structs", {})) for m in modules.values())
print(f"    Modules:    {len(module_names)}")
print(f"    Structs:    {struct_count}")
print(f"    Functions:  {fn_count}")
if module_names:
    print(f"    Module names: {', '.join(module_names[:10])}...")

print("\n" + "=" * 60)
print("All tests passed!")
print("=" * 60)
