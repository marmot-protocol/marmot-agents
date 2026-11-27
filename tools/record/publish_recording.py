"""Where record_codex.py, record_acp.py and record_pi.py write a raw recording, and when it is published.

The recording is written owner-only (0600) in an owner-only folder (0700) beside the destination, and
renamed over the destination only when the scenario finished. A run that fails or is interrupted removes
it and leaves the destination as it was, as publish-recording.sh does for the gateway recorders.
"""

import os
import shutil
import tempfile
from contextlib import contextmanager
from pathlib import Path


@contextmanager
def recording(dest: Path):
    """Yield the path to write the raw recording to. It replaces `dest` only if the block returns."""
    dest = Path(dest)
    work = Path(tempfile.mkdtemp(prefix=f".{dest.name}.", dir=dest.parent))
    try:
        raw = work / dest.name
        raw.touch(mode=0o600)  # opening it to write keeps its mode
        yield raw
        os.replace(raw, dest)
    finally:
        shutil.rmtree(work, ignore_errors=True)
