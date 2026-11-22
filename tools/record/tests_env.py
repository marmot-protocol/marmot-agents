"""The model the recorders run on (D-100), as the scenarios read it: MODEL_BASE_URL, MODEL_NAME and
MODEL_KEY_FILE, from the environment, or else from tests.env at the top of the repository, which git
ignores (tests.env.example shows it). There is no default."""

import os
import sys
from pathlib import Path

NAMES = ("MODEL_BASE_URL", "MODEL_NAME", "MODEL_KEY_FILE")
FILE = Path(__file__).resolve().parents[2] / "tests.env"


def load(path: Path = FILE) -> None:
    """Puts the names tests.env gives into the environment, where the environment has none."""
    try:
        lines = path.read_text().splitlines()
    except FileNotFoundError:
        return
    for line in lines:
        name, sep, value = line.strip().partition("=")
        if sep and name in NAMES and value.strip() and not os.environ.get(name):
            os.environ[name] = value.strip()


def need(*names: str) -> None:
    """Stops, saying what to set, when one of `names` is not set."""
    for name in names:
        if not os.environ.get(name):
            sys.exit(f"error: set {name}, in the environment or in tests.env (see tests.env.example)")
