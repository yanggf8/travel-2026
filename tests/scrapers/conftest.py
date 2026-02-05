"""
Shared fixtures for scraper tests.

Loads raw_text from real scraped JSON files in data/ as test fixtures.
"""

import json
import os
import sys

import pytest

# Add scripts/ to path so we can import scrapers package
_scripts_dir = os.path.join(os.path.dirname(__file__), "..", "..", "scripts")
if _scripts_dir not in sys.path:
    sys.path.insert(0, _scripts_dir)

DATA_DIR = os.path.join(os.path.dirname(__file__), "..", "..", "data")


def _load_fixture(filename: str) -> dict:
    path = os.path.join(DATA_DIR, filename)
    if not os.path.exists(path):
        pytest.skip(f"Fixture file not found: {path}")
    with open(path, encoding="utf-8") as f:
        return json.load(f)


@pytest.fixture
def besttour_data():
    return _load_fixture("besttour-TYO06MM260213AM2.json")


@pytest.fixture
def lifetour_data():
    return _load_fixture("lifetour-test-parsed.json")


@pytest.fixture
def settour_data():
    return _load_fixture("settour-osaka-kansai.json")
