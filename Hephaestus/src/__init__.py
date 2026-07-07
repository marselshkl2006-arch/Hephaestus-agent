"""Claude Code - Python Implementation

AI Coding Assistant CLI with beautiful UI and 35+ tools.
"""

__version__ = "2.0.0"
__author__ = "marselshkl2006-arch"
__all__ = []

# Гефест — основной агент
try:
    from .hephaestus import HephaestusAgent, main as hephaestus_main
except ImportError:
    pass
