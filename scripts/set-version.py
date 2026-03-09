# Project:   hyperi-rustlib
# File:      scripts/set-version.py
# Purpose:   Update VERSION file and Cargo.toml during semantic-release
# Language:  Python
#
# License:   FSL-1.1-ALv2
# Copyright: (c) 2026 HYPERI PTY LIMITED

import re
import sys
from pathlib import Path

version = sys.argv[1]

Path("VERSION").write_text(f"{version}\n")

content = Path("Cargo.toml").read_text()
content = re.sub(
    r'^version = ".*"', f'version = "{version}"', content, flags=re.MULTILINE
)
Path("Cargo.toml").write_text(content)
