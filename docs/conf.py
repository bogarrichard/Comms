"""Sphinx configuration for the Comms documentation.

Build locally (see development.md):

    python3 -m venv .venv && .venv/bin/pip install -r docs/requirements.txt
    .venv/bin/sphinx-build -W --keep-going -b html docs docs/_build/html
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def _workspace_version() -> str:
    """The version in [workspace.package] of Cargo.toml, so docs never drift from releases."""
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    section = re.search(r"^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
    found = re.search(r'^version\s*=\s*"([^"]+)"', section.group(1), re.M) if section else None
    return found.group(1) if found else "unknown"


project = "Comms"
author = "Comms contributors"
copyright = "Comms contributors. Apache-2.0"
release = version = _workspace_version()

extensions = ["myst_parser", "sphinx_copybutton"]
source_suffix = {".md": "markdown"}
root_doc = "index"
exclude_patterns = ["_build", ".venv", "requirements.txt"]

# Markdown: `:::` fences for admonitions, and anchors for #links to headings.
myst_enable_extensions = ["colon_fence"]
myst_heading_anchors = 3

html_theme = "furo"
html_title = f"Comms {release}"
html_theme_options = {
    "source_repository": "https://github.com/bogarrichard/Comms/",
    "source_branch": "main",
    "source_directory": "docs/",
    # The same purple (Teams) and blue (Signal) as the terminal client.
    "light_css_variables": {
        "color-brand-primary": "#7F5FD0",
        "color-brand-content": "#3A76F0",
    },
    "dark_css_variables": {
        "color-brand-primary": "#A58BE6",
        "color-brand-content": "#7BA3F7",
    },
}
