#!/usr/bin/env python3
"""
Check Playwright Installation

Quick check if Playwright is installed and working.
Used by postinstall hook and before scraper runs.

Usage:
    python scripts/check_playwright.py
    python scripts/check_playwright.py --install  # Auto-install if missing
"""

import subprocess
import sys


def check_playwright() -> bool:
    """Check if Playwright is installed."""
    try:
        import playwright
        return True
    except ImportError:
        return False


def check_chromium() -> bool:
    """Check if Chromium browser is installed."""
    try:
        from playwright.sync_api import sync_playwright
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True)
            browser.close()
        return True
    except Exception:
        return False


def install_playwright():
    """Install Playwright and Chromium."""
    print("📦 Installing Playwright...")
    result = subprocess.run(
        [sys.executable, "-m", "pip", "install", "playwright"],
        capture_output=True,
        text=True
    )
    if result.returncode != 0:
        print(f"❌ Failed to install playwright: {result.stderr}")
        return False

    print("📦 Installing Chromium browser...")
    result = subprocess.run(
        ["playwright", "install", "chromium"],
        capture_output=True,
        text=True
    )
    if result.returncode != 0:
        print(f"❌ Failed to install chromium: {result.stderr}")
        return False

    print("✅ Playwright installed successfully!")
    return True


def main():
    import argparse
    parser = argparse.ArgumentParser(description="Check Playwright installation")
    parser.add_argument("--install", action="store_true", help="Install if missing")
    parser.add_argument("--quiet", "-q", action="store_true", help="Quiet mode (exit code only)")
    args = parser.parse_args()

    playwright_ok = check_playwright()

    if not playwright_ok:
        if not args.quiet:
            print("❌ Playwright not installed")
        if args.install:
            if install_playwright():
                sys.exit(0)
            else:
                sys.exit(1)
        else:
            if not args.quiet:
                print("\n  Run: pip install playwright && playwright install chromium")
                print("  Or:  python scripts/check_playwright.py --install")
            sys.exit(1)

    chromium_ok = check_chromium()

    if not chromium_ok:
        if not args.quiet:
            print("⚠️ Playwright installed but Chromium browser missing")
        if args.install:
            print("📦 Installing Chromium browser...")
            result = subprocess.run(
                ["playwright", "install", "chromium"],
                capture_output=True,
                text=True
            )
            if result.returncode != 0:
                print(f"❌ Failed: {result.stderr}")
                sys.exit(1)
            print("✅ Chromium installed!")
        else:
            if not args.quiet:
                print("\n  Run: playwright install chromium")
            sys.exit(1)

    if not args.quiet:
        print("✅ Playwright and Chromium ready")
    sys.exit(0)


if __name__ == "__main__":
    main()
