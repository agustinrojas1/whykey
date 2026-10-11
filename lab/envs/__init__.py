"""Lab environment plugins: one module per scenario "environment".

Every module lab/envs/<name>.py (names starting with "_" excluded) is a
plugin for scenarios with "environment": "<name>"; the module docstring
documents its scenario fields. lab/driver.py calls the hooks with its `Lab`
(the scenario, `lab.env` exported to whykey and the actions, `spawn`, `run`,
`ok`, `ensure_alive`, `steps`, `settle`, `pty`).

    start(lab)                  required: bring the session up and export
                                its variables into lab.env
    stop(lab)                   before the spawned processes are killed
    inject(lab, keys)           type key chords ("shift", "super+t") with
                                the environment's own injector; the Wayland
                                probe's Shift tap uses it
    focused_window(lab)         (app id, title) of the focused window, or
                                ("", ""); without it the probe skips its
                                focus wait and `ready: window` never holds
    logs(lab)                   extra {name: text} for the result's logs
    ACTIONS = {key: fn}         actions {key: ...}: fn(lab, action); the
                                driver then records the step and settles
    CHECKS = {key: fn}          ground-truth checks {key: ...}:
                                fn(lab, spec) -> (bool, evidence)
    VM = {...}                  run the scenario in a KVM guest booted in
                                the container: "qemu_opts", "memory", "cpus"
    docker_args()               host side (lab/run.py): extra `docker run`
                                arguments

All hooks are optional except start. Stdlib only: lab/run.py imports the
plugins on the host.
"""

from __future__ import annotations

import importlib
import pkgutil
from types import ModuleType


def _discover() -> dict[str, ModuleType]:
    environments = {}
    for info in sorted(pkgutil.iter_modules(__path__), key=lambda info: info.name):
        if info.name.startswith("_"):
            continue
        module = importlib.import_module(f"{__name__}.{info.name}")
        if not callable(getattr(module, "start", None)):
            raise ImportError(f"lab/envs/{info.name}.py defines no start(lab)")
        environments[info.name] = module
    return environments


ENVIRONMENTS = _discover()
