"""
Root conftest — adds scripts/ to sys.path for Python scraper tests.
"""

import os
import sys

_scripts_dir = os.path.join(os.path.dirname(__file__), "scripts")
if _scripts_dir not in sys.path:
    sys.path.insert(0, _scripts_dir)
