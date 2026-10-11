# whykey reality lab

The lab checks whykey against the real programs it reports on. Each scenario
starts a real compositor, window manager, hotkey daemon, shell, or
multiplexer in a fresh container. It sends synthetic input there,
records what the program actually did, and then runs whykey. The scenario
passes only when whykey's verdict matches that reaction. Support claims in
[SUPPORT_MATRIX.md](../SUPPORT_MATRIX.md) come from passing scenarios.

Synthetic input (wtype, xdotool, the lab's virtual keyboard) is sent only
inside lab containers. Never send it to your own desktop session.

## Requirements

- Docker. Each scenario runs in its own `docker run --rm --network none --init`
  container.
- A whykey release build (`cargo build --release --locked`).
- `/dev/kvm` for the Hyprland scenarios (see below). The rest need no KVM.
- About 3 GB of disk space for the `whykey-lab` image.

## Build and run

```sh
docker build -t whykey-lab lab/
cargo build --release --locked
python3 lab/run.py -j 12
```

`run.py` is stdlib-only Python and runs on the host. It prints one line per
scenario, then a table of ground truth against whykey's verdict, followed by
details for every failure. It writes `lab/results.json` (git-ignored) and
exits 1 on any mismatch.

| Option | Effect |
| --- | --- |
| `--only GLOB` | Run matching scenario ids or `scenarios/`-relative paths. Repeatable, e.g. `--only 'i3/*' --only sway-release-bind`. |
| `-j N` | Run N containers at once. Default: 4, or fewer if there are fewer CPUs. |
| `--vms N` | Run at most N Hyprland VMs at once (2 vCPUs each, within `-j`). Default: a third of the CPUs when `/dev/kvm` exists, a quarter without. Starved VMs time out. |
| `--binary PATH` | Test this whykey binary. Default: `target/release/whykey`. Pin a build when other work keeps rebuilding `target/`. |
| `--keep` | Leave each container running afterwards, for `docker exec`. |
| `--image NAME` | Use this lab image. Default: `whykey-lab`. |
| `--results FILE` | Write the results to this file. Default: `lab/results.json`. |
| `--repeat N` | Run every selected scenario N times at once, to hunt flakes, e.g. `--only sway-mode-inactive --repeat 30 -j 12`. |

To add a run's results to the support matrix, run
`python3 tools/generate_support_matrix.py` (it reads `lab/results.json`) or
pass `--lab-results FILE`. The CI workflow `.github/workflows/lab.yml` builds
the image with layer caching, runs every scenario on pull requests, and
uploads `results.json`.

## The image

`lab/Dockerfile` builds Fedora 44 with pinned versions: Sway 1.11, i3
4.25.1, Openbox 3.6.1, AwesomeWM 4.3, sxhkd 0.6.1, xbindkeys 1.8.7,
kglobalacceld 6.7.5, mutter 50 (headless) with the GNOME schemas, and
python3-dbus. It also has bash
5.3, zsh 5.9, fish 4.6, tmux 3.7c, Screen 5.0, Xvfb, xdotool, xev, wtype,
foot, and Hyprland 0.56.2 from the `sdegler/hyprland` COPR (Fedora no longer
ships Hyprland). If the COPR drops that build, the image build fails; it
does not silently switch versions. File capabilities are stripped from every
binary, because they make `exec` fail with EPERM in an unprivileged
container.

The image also has Emacs 30.2 (`emacs-nw`, `emacsclient`) and Neovim 0.12.5;
dwl v0.9, built from source against wlroots 0.20; Niri, labwc, and Wayfire
from the Fedora repositories (unpinned); Ghostty and WezTerm from the
`scottames/ghostty` and `wezfurlong/wezterm-nightly` COPRs; kitty, alacritty,
foot, and Konsole; and Qtile, XMonad, Xfce, MATE, and herbstluftwm. Cinnamon
is installed but not verified: `cinnamon --replace` fails headless, and muffin
has no standalone binary.

Packages come in families under `lab/packages/`: `<family>.txt` lists dnf
package names, one per line (`#` comments allowed, pin versions as
`name-1.2.3`), and an optional `<family>.sh` runs before the install (enable
a COPR, as `hyprland.sh` does). `_install.sh` installs one family, then
strips file capabilities and recompiles the GSettings schemas. The
Dockerfile installs `base` first and then every other family in name order,
one `COPY packages/<family>.* /tmp/packages/` + `RUN sh
/tmp/packages/_install.sh <family>` pair each, so changing one family
rebuilds only its layer and those after it. A new family adds its pair at
its sorted place.

## How ground truth is established

whykey's own probes are never the ground truth. Each scenario records the
program's reaction through an independent path:

- **Did the binding fire?** The binding runs a command that leaves a trace,
  usually `touch /tmp/fired`, and the check looks for that trace. Other
  checks: a file's contents, or any shell command whose exit status
  decides.
- **Did the key go on?** An independent client with focus records the keys
  it receives: foot running `lab/keylog.py` with the Kitty keyboard protocol
  on Wayland, or `xev` on X11.
- **Does the probe have focus?** On Wayland (Sway, Hyprland, GNOME) the
  driver types a Shift tap with the scenario's own injector (`vkbd` or
  `rdkbd`) and waits until foot receives the Shift *press* before any
  action. A lone release does not count: it shows up when the press was
  lost. Focus as reported by the compositor was not enough: under load it
  let a scenario pass without proving anything.
- **No lost first key.** Every `vkbd` run creates a new virtual keyboard.
  The first event from a new keyboard makes the compositor switch the seat
  to it and send clients its keymap, and on Sway a key press that is that
  first event could be lost (foot saw only the release). `vkbd` therefore
  sends a no-op modifiers event and waits 150 ms before typing. Mutter
  drops the first key press of every new `rdkbd` remote-desktop session,
  whatever the delay, so `rdkbd` spends it on a Shift tap; clients see a
  stray Shift release.
- **Did the key arrive anywhere?** `reached` is the positive control: the
  bind fired, or a check such as `{"probe": "any"}` (the probe received any
  key press, modifiers included) shows that the input reached the session.
  Every scenario needs `delivered` or `reached`.

The truth maps to the accepted whykey verdict for the layer under test:

| Ground truth | Result |
| --- | --- |
| fired, key not delivered | `consumed` |
| fired, key delivered | `handled-passed` |
| fired, no probe | `consumed` or `handled-passed` |
| not fired, key delivered or reached | `pass` |
| not fired, not delivered, reached | FAIL: something whykey does not model withheld the key |
| not fired, not delivered, not reached | LAB ERROR: the setup proved nothing, never a pass |

Shells and other last consumers have no `delivered`; they need `reached`.
There, fired (including `self-insert`) means `consumed`, and not fired but
reached means `pass`.

Verdicts come from the layer's status and propagation pair: Handled+Stops is
`consumed`, Handled+Continues is `handled-passed`, NotHandled+Continues is
`pass`, Handled+Indeterminate is `handled-uncertain`, and
Indeterminate+Continues is `uncertain-continues`.

Input sources and what they can prove:

- `vkbd` (`lab/vkbd.py`) is a Wayland virtual keyboard with the real US
  keymap and real evdev keycodes. Use it for `--to-code` binds, for
  Hyprland's native capture, and for keypad keys (`kp_0`–`kp_9`,
  `kp_enter`, `kp_add`, `kp_subtract`, `kp_multiply`, `kp_divide`,
  `kp_decimal`; `rdkbd` shares the table). `vkbd.py --numlock KEY…` types
  with NumLock locked (Mod2 in the locked modifiers).
- `wtype` uploads its own private keymap. Sway `--to-code` binds do not
  fire for it, and Hyprland's Lua key hook sees every wtype key as keycode
  9, which the real layout reads as Escape (D-073, D-089).
- `qkbd` (`lab/qkbd.py`) is the kernel keyboard of the Hyprland VM. `lab/qkbd.py`
  drives it over QMP; its key requests reach the guest through the
  `whykey.keyboard` virtio-serial port, which `keyboard_relay` in
  `lab/driver.py` relays. The keys are real kernel input events
  (`hyprland-qkbd-*`).
- `xdotool` sends XTEST input on X11. `lab/xgrab.py` confirms over the raw X11
  socket that a daemon holds its grab before input is sent. Two traps:
  `xdotool key Super_L+t` releases Super before `t`, so an sxhkd `@`
  (release) binding never matches; send `keydown Super_L keydown t keyup t
  keyup Super_L` instead. In Xvfb's keymap `xdotool key F5` also presses
  Alt_L; send the keycode (`xdotool key 71`). sxhkd needs `SHELL` in its
  environment to run commands.
- `type` writes bytes to a pty. The pty driver answers terminal queries;
  without that, fish 4.6 waits 10 s and swallows input.

GNOME scenarios run headless mutter 50 (`mutter --headless --wayland --no-x11
--virtual-monitor`) and send input through mutter's RemoteDesktop D-Bus API
(`rdkbd`, `lab/rdkbd.py`). GNOME Shell cannot start in the container (it
aborts without systemd-logind), so they verify only bindings that mutter
owns. For example, the close binding (default `<Alt>F4`, or rebound to
`<Super>F9`) closes the foot probe, and unbound keys reach it. GNOME Shell
and gnome-settings-daemon behaviour is not verified; the support matrix
says so in its `lab_scope`.

The KDE scenarios run a bare kglobalacceld on Xvfb. For the inactive
component, the "fired" truth is `lab/xgrab.py` showing that kglobalacceld
holds no X11 grab on Meta+Y.

Scenarios ending in `-no-display` (i3 and KDE) run whykey with `DISPLAY`
unset, so the X11 grab probe cannot decide the verdict and the adapter's own
logic must. The support matrix credits grab-decided verdicts to
`compositor.x11-grab`.

## Scenario format

One JSON file per scenario, `lab/scenarios/<group>/<id>.json`. The file stem
must equal `id`. The full format, with every action and check, is in the
docstring of `lab/run.py`. A minimal scenario:

```json
{
  "id": "sway-plain-bind",
  "description": "bindsym Mod4+t fires and Sway consumes the key",
  "tier": "observed",
  "environment": "sway",
  "files": {"/root/.config/sway/config": ["bindsym Mod4+t exec touch /tmp/fired"]},
  "probe": "wayland",
  "actions": [{"vkbd": ["super+t"]}],
  "ground_truth": {"fired": {"file": "/tmp/fired"}, "delivered": "probe"},
  "whykey": {
    "args": ["super+t"],
    "layer": "Sway",
    "env": {"XDG_CURRENT_DESKTOP": "sway", "XDG_SESSION_TYPE": "wayland"}
  }
}
```

| Field | Meaning |
| --- | --- |
| `tier` | Always `observed`: a program reacted to input |
| `environment` | Name of an environment plugin, `lab/envs/<name>.py`: `sway` (headless), `hyprland` (KVM guest), `gnome` (headless mutter), `x11` (Xvfb `:5` plus `x11.daemons` started in order and waited on), `pty` (`pty.command` on a pseudo-terminal), ...; the plugin's docstring documents its own fields. A session D-Bus is always started |
| `files`, `setup`, `session_setup` | Files written before start, and shell commands run before and after the environment comes up |
| `probe` | Independent key recorder: `wayland` or `x11` |
| `actions` | `vkbd`, `rdkbd`, `wtype`, `xdotool`, `type`, `sh`, `sleep`, `wait_quiet`, `start_whykey`, or one of the environment's own `ACTIONS` |
| `ground_truth` | `fired` (did the layer act), `delivered` (`probe`, or a check), and `reached` (`fired`, or a check such as `{"probe": "any"}`); `delivered` or `reached` is required |
| `whykey.args`, `layer`, `env`, `run` | Command line, layer under test, environment, and how to run it: `direct`, `pty` (typed into the scenario's shell), or `background` (`listen`, with `ready`). For a key sequence (several key arguments) the verdict is the last key's (`steps[-1].path`) |
| `whykey.summary`, `detail`, `binding`, `combo`, `stderr` | Substrings or values that must also match, so a right verdict for the wrong binding still fails. `binding` is a substring of the reported layer's primary binding action |
| `whykey.observation` | Judge `listen`'s observation (`delivered` or not) instead of a layer |
| `whykey.accept` | Honest-uncertainty verdicts tolerated when the scenario documents that whykey cannot know more (for example, a shell without `shell-init`). Shown as `PASS~` and counted separately in the run summary; such passes never count as verification. |
| `whykey.confidence` | The report's `assessment.confidence` must equal this value |

Probe-window scenarios send the same input twice. The first time, the lab's
own probe client has focus. The second time, `whykey listen --window` has
focus. The bind must fire both times, and whykey's observation must match
what the independent client saw.

## Environment plugins

Each `lab/envs/<name>.py` (names starting with `_` excluded) is the
environment `<name>`; `lab/envs/__init__.py` imports them all, and both
`run.py` (host) and `driver.py` (container) use that registry, so a new
environment is one new file plus its package family. Plugins are stdlib
only and share `lab/common.py` (`LabError`, `wait_for`, paths). Every hook
gets the driver's `Lab`: `lab.scenario`, `lab.env` (exported to whykey,
actions and probes), `lab.spawn(name, argv, env)`, `lab.run(argv)`,
`lab.ok(argv)`, `lab.ensure_alive()`, `lab.steps`, `lab.settle`, `lab.pty`.

Registered environments: `dwl`, `emacs`, `gnome`, `hyprland`, `labwc`, `niri`,
`nvim`, `pty`, `sway`, `terminals`, `wayfire`, and `x11`.

| Hook | Use |
| --- | --- |
| `start(lab)` | Required. Start the session, wait until it is ready, export its variables into `lab.env` |
| `stop(lab)` | Teardown before the spawned processes are killed |
| `inject(lab, keys)` | Type chords (`"shift"`, `"super+t"`) with the environment's own injector; the Wayland probe's Shift tap uses it |
| `focused_window(lab)` | `(app id, title)` of the focused window, or `("", "")`. Without it the probe skips its focus wait (the Shift tap still has to arrive) and `ready: window` never holds |
| `logs(lab)` | Extra `{name: text}` for the result's logs |
| `ACTIONS = {key: fn}` | Scenario actions `{key: ...}` beyond the generic ones; `fn(lab, action)`, after which the driver records the step and waits `settle` |
| `CHECKS = {key: fn}` | Ground-truth checks `{key: ...}` beyond `file`, `sh`, and `probe`; `fn(lab, spec)` returns `(bool, evidence)` |
| `VM = {...}` | Run the scenario in a KVM guest booted in the container: `qemu_opts`, `memory` (default `2G`), `cpus` (default 2). `run.py` runs these within `--vms` |
| `docker_args()` | Host side: extra `docker run` arguments, e.g. `--device /dev/kvm` |

## Hyprland and KVM

Hyprland does not run in a plain container: its backend library,
aquamarine, needs a KMS device even for headless outputs. For each
`hyprland` scenario, `lab/driver.py` uses virtme-ng to boot the image's own
kernel in a QEMU guest. The container is the guest's root filesystem over
9p, virtio-gpu provides KMS, and Mesa's llvmpipe renders. The driver then
runs the scenario again inside the guest and returns the result through a
shared directory. Hyprland runs as root there (`--i-am-really-stupid`),
which is acceptable only because the guest is thrown away.

`run.py` passes `/dev/kvm` to the container when the host has it. Without
it the guest falls back to TCG emulation, which is much slower and can time
out. Check access with `ls -l /dev/kvm`; your user usually needs to be in
the `kvm` group. GitHub's Linux runners provide `/dev/kvm`. Expect about
12 s per Hyprland scenario with KVM.

A VM spec with `keyboard: true` replaces the guest's keyboard with the `qkbd`
virtio-keyboard (`-machine i8042=off`), so every key the guest receives comes
from `qkbd`.

## Debugging a scenario

```sh
python3 lab/run.py --only sway-plain-bind --keep --results /tmp/r.json
docker exec -it whykey-lab-<pid>-sway-plain-bind bash
```

Logs of the started programs are in `/tmp/lab-logs` inside the container.
The kept container's name is printed at the end of the run. Remove it with
`docker rm -f` when you are done.

## Mutation check

`lab/mutate.py` shows which scenarios catch a deliberate whykey bug. A
mutation no scenario catches is a coverage gap. It does not run in CI.

```sh
python3 lab/mutate.py lab/mutations.example.json --only 'sway/*' -j 12 [--rev HEAD]
```

The mutation file is a JSON list of `{"name", "file", "search", "replace"}`
objects. `search` must occur exactly once in `file`. The tool copies the
tracked files (or `git archive REV` with `--rev`, useful while the working
tree does not build) to `target/mutate/src`. It builds an unmutated
baseline, then each mutant, with `CARGO_TARGET_DIR=target/mutate/target`.
Before every build it runs `cargo clean -p whykey`, because cargo can keep a
stale whykey artifact between source copies. A mutant whose binary is
identical to the baseline is reported as such. Scenarios that fail on the
baseline count as noise, not catches. Lab errors are listed separately
("lab errors (not counted)") and never count as catches. The results file
is deleted before every run, and the tool stops if `run.py` exits with a
status other than 0 or 1 or writes no results. Binaries and per-run results stay in
`target/mutate`; delete the directory when you are done (it holds about
600 MB).
