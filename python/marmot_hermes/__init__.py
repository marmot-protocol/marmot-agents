"""The Marmot platform for Hermes: a Hermes plugin that connects Hermes to marmot-agent's local socket.

Hermes loads this folder as a plugin and calls `register`. Everything that needs Hermes is in
`adapter.py`, imported only here, so the translator and the connector can be tested without Hermes.
"""

__version__ = "0.1.0"


def register(ctx):
    """Hermes's plugin entry point."""
    from .adapter import register as register_platform

    register_platform(ctx)
