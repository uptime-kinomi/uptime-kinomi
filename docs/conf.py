"""Sphinx configuration of the uptime-kinomi documentation.

    pip install -r docs/requirements.txt
    sphinx-build -W --keep-going docs docs/_build/html
"""
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parent.parent

project = "uptime-kinomi"
author = "the uptime-kinomi authors"
copyright = "2026, the uptime-kinomi authors"
# The version of the code next to these docs (Read the Docs builds every tag).
release = re.search(r'^version = "(.+)"$', (ROOT / "Cargo.toml").read_text(), re.M).group(1)
version = release

extensions = [
    "myst_parser",  # only to include SPEC.md (reference/specification.rst)
]
source_suffix = {".rst": "restructuredtext"}
exclude_patterns = ["_build", "examples"]
# SPEC.md and CHANGELOG.md start at "##" below the page title.
suppress_warnings = ["myst.header"]
myst_heading_anchors = 3

highlight_language = "none"
html_theme = "furo"
html_title = f"uptime-kinomi {release}"
html_logo = "logo.svg"
html_favicon = "../assets/favicon.svg"
html_static_path = ["_static"]
html_theme_options = {
    "source_repository": "https://github.com/uptime-kinomi/uptime-kinomi/",
    "source_branch": "main",
    "source_directory": "docs/",
    "light_css_variables": {
        "color-brand-primary": "#62268f",
        "color-brand-content": "#62268f",
    },
    "dark_css_variables": {
        "color-brand-primary": "#b88ae0",
        "color-brand-content": "#b88ae0",
    },
}
html_show_sourcelink = False
