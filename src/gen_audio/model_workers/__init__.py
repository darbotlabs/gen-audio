"""Isolated, offline model workers (Python 3.12), not app-environment backends.

Run ``python -m gen_audio.model_workers --help`` with this repository's ``src``
on PYTHONPATH. Install exactly one runtimes/requirements-*.txt into a separate
Python 3.12 environment; do not install these requirements into the desktop app.
Preflight uses only the standard library and never imports a model runtime.
"""
