#!/usr/bin/env python3
"""Fail when a crate has less than the minimum line coverage.

Usage: coverage-gate.py <llvm-cov export JSON> <minimum percent>

The script counts the lines of `crates/<name>/src/` for each crate. It
ignores tests and examples. Each crate must reach the minimum on its own.
"""

import json
import re
import sys

CRATES = [
    "autumn-jira",
    "autumn-jira-fake",
    "autumn-plugin-jira",
    "autumn-jira-harvest",
    "autumn-jira-camel",
]


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    with open(sys.argv[1], encoding="utf-8") as handle:
        report = json.load(handle)
    minimum = float(sys.argv[2])
    totals = {name: [0, 0] for name in CRATES}
    pattern = re.compile(r"/crates/([^/]+)/src/")
    for data in report["data"]:
        for entry in data["files"]:
            found = pattern.search(entry["filename"])
            if not found or found.group(1) not in totals:
                continue
            lines = entry["summary"]["lines"]
            totals[found.group(1)][0] += lines["covered"]
            totals[found.group(1)][1] += lines["count"]
    failed = False
    for name, (covered, count) in totals.items():
        if count == 0:
            print(f"FAIL {name}: no lines found in the report")
            failed = True
            continue
        percent = 100.0 * covered / count
        state = "ok  " if percent >= minimum else "FAIL"
        failed |= percent < minimum
        print(f"{state} {name}: {percent:.2f}% ({covered}/{count} lines)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
