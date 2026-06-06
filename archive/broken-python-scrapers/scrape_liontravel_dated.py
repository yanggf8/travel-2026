#!/usr/bin/env python3
"""Legacy LionTravel Python scraper stub.

LionTravel is decommissioned from the Python parser path. Use the Rust
travel-scraper CDP flow (`scrape ...` then `parse capture ...`) instead.
"""

import sys


def main() -> int:
    print(
        "LionTravel Python scraper is decommissioned; use rust/target/debug/travel-scraper",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
