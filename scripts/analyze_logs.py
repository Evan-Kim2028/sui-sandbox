import json
import sys
from collections import defaultdict


def analyze_log(file_path):
    print(f"--- Analyzing {file_path} ---")

    stats = {
        "total_packages": 0,
        "dry_run_ok": 0,
        "max_planning_calls_exceeded": 0,
        "schema_violations": 0,
        "harness_errors": 0,
        "total_planning_calls": 0,
        "packages_with_need_more": 0,
        "total_need_more_events": 0,
        "errors": defaultdict(int),
    }

    package_calls = defaultdict(int)

    try:
        with open(file_path, "r") as f:
            for line in f:
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue

                event_type = event.get("event")
                pkg_id = event.get("package_id")

                if event_type == "llm_response":
                    package_calls[pkg_id] += 1
                    stats["total_planning_calls"] += 1

                    # Check for need_more usage
                    content = event.get("content", "")
                    parsed = event.get("parsed")  # Some logs have parsed field

                    used_need_more = False
                    if parsed and isinstance(parsed, dict) and "need_more" in parsed:
                        used_need_more = True
                    elif '"need_more":' in content:
                        used_need_more = True

                    if used_need_more:
                        stats["total_need_more_events"] += 1

                if event_type == "package_finished":
                    stats["total_packages"] += 1
                    if event.get("dry_run_ok"):
                        stats["dry_run_ok"] += 1

                    error = event.get("error")
                    if error:
                        stats["errors"][error[:50]] += 1  # truncated error for summary
                        if "schema violations" in error:
                            stats["schema_violations"] += 1

                if event_type == "plan_attempt_harness_error":
                    error = event.get("error")
                    if error and "max planning calls exceeded" in error:
                        stats["max_planning_calls_exceeded"] += 1
                    else:
                        stats["harness_errors"] += 1

        print(json.dumps(stats, indent=2, default=str))

        if stats["total_packages"] > 0:
            avg_calls = stats["total_planning_calls"] / stats["total_packages"]
            print(f"Average Planning Calls per Package: {avg_calls:.2f}")
            print(
                f"Success Rate: {(stats['dry_run_ok'] / stats['total_packages']) * 100:.1f}%"
            )

    except FileNotFoundError:
        print(f"File not found: {file_path}")


if __name__ == "__main__":
    for arg in sys.argv[1:]:
        analyze_log(arg)
