#!/usr/bin/env python3
"""
Deprecated.

Package filtering must query Turso-imported offers. Raw scraper files are only
allowed as an import landing zone.
"""

import sys


def main() -> None:
    print(
        "filter_packages.py is disabled: import scraper output with "
        "npm run db:import:turso, then use npm run db:query:turso or "
        "npm run travel -- compare-offers.",
        file=sys.stderr,
    )
    sys.exit(1)


if __name__ == "__main__":
    main()
