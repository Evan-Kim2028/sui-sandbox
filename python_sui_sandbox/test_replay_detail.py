"""Deeper replay analysis — pick a transaction with MoveCall commands."""

import sui_move_extractor

latest = sui_move_extractor.get_latest_checkpoint()
cp = sui_move_extractor.get_checkpoint(latest)

# Find a transaction with commands > 0
target_tx = None
for tx in cp["transactions"]:
    if tx["commands"] > 0:
        target_tx = tx
        break

if not target_tx:
    print("No programmable transactions in this checkpoint, trying checkpoint - 1")
    cp = sui_move_extractor.get_checkpoint(latest - 1)
    for tx in cp["transactions"]:
        if tx["commands"] > 0:
            target_tx = tx
            break

if not target_tx:
    print("Could not find a non-system transaction. Exiting.")
    exit(0)

digest = target_tx["digest"]
checkpoint = cp["checkpoint"]
print(f"Checkpoint: {checkpoint:,}")
print(f"Digest:     {digest}")
print(f"Sender:     {target_tx['sender']}")
print(f"Commands:   {target_tx['commands']}")
print()

# Verbose replay
info = sui_move_extractor.walrus_analyze_replay(digest, checkpoint, verbose=True)

print(f"Objects hydrated:  {info['objects']}")
print(f"Packages loaded:   {info['packages']} ({info['modules']} modules)")
print(f"Epoch: {info['epoch']}  Protocol: {info['protocol_version']}")
print()

print("Commands:")
for i, cmd in enumerate(info.get("command_summaries", [])):
    target = cmd.get("target", "")
    print(f"  [{i}] {cmd['kind']}", end="")
    if target:
        print(f"  -> {target}", end="")
    print(f"  (type_args={cmd['type_args']}, args={cmd['args']})")

print("\nInput Summary:")
for k, v in info.get("input_summary", {}).items():
    print(f"  {k}: {v}")

if "input_objects" in info:
    print(f"\nInput Objects ({len(info['input_objects'])}):")
    for obj in info["input_objects"][:10]:
        print(f"  {obj['kind']:12s}  {obj['id']}")

if "package_ids" in info:
    print(f"\nPackage IDs ({len(info['package_ids'])}):")
    for pid in info["package_ids"]:
        print(f"  {pid}")

if "object_types" in info:
    print(f"\nObject Types ({len(info['object_types'])}):")
    for obj in info["object_types"][:10]:
        shared = " [shared]" if obj.get("shared") else ""
        immut = " [immutable]" if obj.get("immutable") else ""
        print(f"  {obj['id'][:24]}...  v{obj['version']}  {obj.get('type_tag', 'n/a')}{shared}{immut}")
    if len(info["object_types"]) > 10:
        print(f"  ... and {len(info['object_types']) - 10} more")
