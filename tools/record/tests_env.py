"""The model the recorders run on (D-100), as the scenarios read it: MODEL_BASE_URL, MODEL_NAME and
MODEL_KEY_FILE, and OPENCODE_MODEL for OpenCode, from the environment, or else from tests.env at the
top of the repository, which git ignores (tests.env.example shows it). There is no default, and a name
the file gives twice is an error."""

import os
import sys
from pathlib import Path

NAMES = ("MODEL_BASE_URL", "MODEL_NAME", "MODEL_KEY_FILE")
# The model as OpenCode names it, for OpenCode, which reaches its models by its own providers.
OPENCODE = "OPENCODE_MODEL"
FILE = Path(__file__).resolve().parents[2] / "tests.env"


def load(path: Path = FILE) -> None:
    """Puts the names tests.env gives into the environment, where the environment has none."""
    try:
        lines = path.read_text().splitlines()
    except FileNotFoundError:
        return
    found: dict[str, str] = {}
    for line in lines:
        name, sep, value = line.strip().partition("=")
        name = name.rstrip()
        if not sep or name not in (*NAMES, OPENCODE):
            continue
        if name in found:
            sys.exit(f"error: {path.name} sets {name} more than once; keep one")
        found[name] = value.strip()
    for name, value in found.items():
        if value and not os.environ.get(name):
            os.environ[name] = value


def need(*names: str) -> None:
    """Stops, saying what to set, when one of `names` is not set."""
    for name in names:
        if not os.environ.get(name):
            sys.exit(f"error: set {name}, in the environment or in tests.env (see tests.env.example)")
