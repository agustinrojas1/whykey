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
- About 10 GB of disk space for the six lab images (see [The images](#the-images)); a scenario runs in one of them, 2.1 to 3.5 GB each.

## Build and run

```sh
python3 tools/lab_images.py build
cargo build --release --locked
python3 lab/run.py -j 12
```

`tools/lab_images.py build` builds every image (or the named ones, e.g.
`build tty`, which builds `base` first if needed) and tags each
`whykey-lab-<name>:<hash>` and `whykey-lab-<name>:latest`. `run.py` is
stdlib-only Python and runs on the host. It prints one line per
scenario, then a table of ground truth against whykey's verdict, followed by
details for every failure. It writes `lab/results.json` (git-ignored) and
exits 1 on any mismatch.

To run part of the lab, combine `--group IMAGE` (the scenarios that run in one
group image, e.g. `vm`) with `--only` and `--shard I/N` (shard I of N of the
selected scenarios, dealt round-robin by sorted id). CI runs one shard per
job; see [Continuous integration](#continuous-integration).

| Option | Effect |
| --- | --- |
| `--only GLOB` | Run matching scenario ids or `scenarios/`-relative paths. Repeatable, e.g. `--only 'i3/*' --only sway-release-bind`. |
| `-j N` | Run N containers at once. Default: 4, or fewer if there are fewer CPUs. |
| `--vms N` | Run at most N Hyprland VMs at once (2 vCPUs each, within `-j`). Default: a third of the CPUs when `/dev/kvm` exists, a quarter without. Starved VMs time out. |
| `--binary PATH` | Test this whykey binary. Default: `target/release/whykey`. Pin a build when other work keeps rebuilding `target/`. |
| `--keep` | Leave each container running afterwards, for `docker exec`. |
| `--image-prefix P` | Image names are `P<name>:<tag>`. Default: `whykey-lab-`. |
| `--image-tag TAG` | Use this tag of each image. Default: `latest`. `hash` uses each image's content hash (`python3 tools/lab_images.py hash NAME`). |
| `--image REF` | Run every scenario in this image reference, as is, instead of its group image. E.g. `--image whykey-lab` for the old single image. |
| `--results FILE` | Write the results to this file. Default: `lab/results.json`. |
| `--repeat N` | Run every selected scenario N times at once, to hunt flakes, e.g. `--only sway-mode-inactive --repeat 30 -j 12`. |
| `--retries N` | Rerun a scenario that ended in a lab error (ground truth could not be established, e.g. a slow terminal lost the first key) up to N times, one at a time. Default: 2. A mismatch with reality is never retried. Retried records carry `retried`. |
| `--group IMAGE` | Only the scenarios that run in this group image: `base`, `wayland`, `tty`, `x11`, `gnome` or `vm` (the `kde` scenarios of `x11` run in `vm`). |
| `--shard I/N` | Only shard I of N of the selected scenarios, sorted by id and dealt round-robin. `tools/lab_select.py` plans the CI shards with the same split. |
| `--merge FILE...` | Runs nothing: merges results files, prints the combined table, writes `--results` and exits 1 on any failure. The CI summary job uses it. |

To add a run's results to the support matrix, run
`python3 tools/generate_support_matrix.py` (it reads `lab/results.json`) or
pass `--lab-results FILE`. CI does not change the support matrix: its results
are run artifacts (see [Continuous integration](#continuous-integration)).

## The images

Every scenario runs in the image of its environment, so it pulls only the
packages that environment needs. `lab/images.json` lists the images, their
package families and their environments; `tools/lab_images.py` builds them.
CI pulls them from GHCR as `ghcr.io/agustinrojas1/whykey-lab-<name>:<hash>`;
`lab-images.yml` builds and pushes the ones whose hash tag is missing.
Each image is Fedora 44. `base` is the shared base, and each group image is
built FROM it (`lab/images/<name>.Dockerfile`, one `COPY` and `RUN` pair per
family, sorted by name).

| Image | Families | Environments | Size (`docker images`) |
| --- | --- | --- | --- |
| `base` | `base`, `wayland`, `x11` | the base of every group | 1.24 GB |
| `wayland` | `ime`, `waylandwms` | `sway`, `niri`, `labwc`, `wayfire`, `dwl`, `ime` | 2.82 GB |
| `tty` | `editors`, `shells`, `termapps`, `terminals` | `pty`, `nvim`, `emacs`, `terminals` | 3.52 GB |
| `x11` | `x11desktops` | `x11` (except its `kde` scenarios), `cinnamon` | 2.99 GB |
| `gnome` | `gnome` | `gnome`, `gnome_shell` | 2.10 GB |
| `vm` | `hyprland`, `kde`, `kwin`, `remap` | `hyprland`, `kwin`, `remap`, and the `kde` scenarios of `x11` | 3.13 GB |

The sizes are uncompressed image sizes; the base layers are shared. The
groups hold their own copies of libraries that other families also pull in,
so the six images together take about 10 GB of disk.

What the images hold:

- `base`: python3 and python3-dbus (the driver and `lab/rdkbd.py`), the
  session dbus, Mesa software rendering and `script` for tty runs; Sway and
  wtype, foot (the Wayland probe) and wev; Xvfb, xdotool, xev, xinput, i3,
  Openbox, AwesomeWM, sxhkd and xbindkeys.
- `wayland`: niri, labwc and Wayfire from the Fedora repositories (unpinned);
  dwl v0.9, built from source against wlroots 0.20; Fcitx5 and IBus with
  pinyin engines.
- `tty`: Emacs 30.2 (`emacs-nw`, `emacsclient`) and Neovim 0.12.5; bash,
  zsh 5.9, fish 4.6, tmux 3.7c and Screen 5.0; Vim, Helix, Kakoune, Micro,
  strace and Zellij 0.45.1 (pinned upstream binary); kitty, alacritty, foot,
  Konsole, and Ghostty and WezTerm from their COPRs.
- `x11`: Qtile, XMonad, Xfce, MATE, herbstluftwm and Cinnamon 6.6
  (`cinnamon --replace` on Xvfb; it needs a system bus with
  `org.freedesktop.locale1`).
- `gnome`: headless mutter 50 with the GNOME schemas, gnome-shell and
  gnome-settings-daemon.
- `vm`: Hyprland 0.56.2 from the `sdegler/hyprland` COPR (Fedora no longer
  ships Hyprland), and the VM tooling (virtme-ng, QEMU, the image's own
  kernel, busybox); KWin 6.7 and kglobalacceld; keyd 2.6.0, kanata 1.12.0,
  xremap 0.15.14, kmonad 0.4.5 (pinned upstream artifacts, sha256 in
  `remap.sh`) and input-remapper 2.2.1.

If a COPR drops the pinned Hyprland build, the image build fails; it does not
silently switch versions. File capabilities are stripped from every binary,
because they make `exec` fail with EPERM in an unprivileged container.

Packages come in families under `lab/packages/`: `<family>.txt` lists dnf
package names, one per line (`#` comments allowed, pin versions as
`name-1.2.3`), and an optional `<family>.sh` runs before the install (enable
a COPR, as `hyprland.sh` does, or install a compiler the family needs, as
`remap.sh` does). `_install.sh` installs one family, then strips file
capabilities and recompiles the GSettings schemas. A family belongs to one
image; `lab/images.json` is checked for that when it is read. A new family
adds its `COPY` and `RUN` pair at its sorted place in its image's Dockerfile.

Building: `python3 tools/lab_images.py build [--prefix P] [NAME...]` builds
the named images (all by default; a group also builds `base` if needed) and
tags each `<P><name>:<hash>` and `<P><name>:latest`. `P` defaults to
`whykey-lab-`. The hash of an image covers its Dockerfile, `_install.sh`, its
families' files and, for a group, the base image's hash, so a package change
rebuilds only the images that contain it. `python3 tools/lab_images.py list`
prints the build order; `hash NAME` and `tag-of NAME` print the hash and the
full tag. `run.py --image-tag hash` runs the images whose content hashes are
those tags. CI builds with `build --push --no-base --label K=V`, which pushes
only the hash tag and does not build the base again; `missing` lists the
images whose hash tag a registry lacks.

## Continuous integration

Three workflows run the lab. It is not a required check: `ci.yml` (`check`,
`msrv`) is.

- **`.github/workflows/lab-images.yml`** builds the images that GHCR lacks and
  pushes them: base first, then the group images as a matrix, each `FROM` the
  base hash. It runs on pushes to main that touch `lab/images/**`,
  `lab/images.json`, `lab/packages/**` or `tools/lab_images.py`, on
  `workflow_dispatch`, and when `lab.yml` calls it. Each image gets the
  `org.opencontainers.image.source` label and only its hash tag. No Actions
  cache is used: the images are the cache.
- **`.github/workflows/lab.yml`** runs on pull requests, nightly (a full run at
  03:17 UTC) and on `workflow_dispatch` (`full: true` runs every scenario). Its
  jobs:
  - `plan` runs `tools/lab_select.py` on the pull request's changed paths and
    emits the shards. A pull request runs the scenarios its changes can affect;
    a change that cannot affect a verdict (documentation, packaging) runs none.
  - `images` calls `lab-images.yml`, so the images for this commit's hashes
    exist. It is skipped when the plan is empty.
  - `lab` runs one job per shard. It pulls the shard's image, builds whykey
    (`cargo build --release --locked`), runs `lab/run.py` with the shard's
    arguments and uploads `results-<shard>.json`. A shard has 30 minutes.
  - `summary` always runs. It merges the shard results, prints the combined
    table in the job summary, and fails on any failed scenario or unfinished
    job. An empty plan succeeds with a note.
- **`tools/lab_select.py`** reads changed paths, one per line, and prints the
  plan as JSON. Its rules are in `lab/affected.json`. `core` paths select every
  scenario. Each `rules` entry maps paths to scenario globs, to a group image
  (`image:tty`), or to the changed scenario, environment plugin, image or
  package family. `ignore` paths select nothing. Any other path selects every
  scenario, so a new source file runs the whole lab until it is mapped. The
  rules are tested by `tools/tests/test_lab_select.py`. To see what a branch
  selects, pipe its changed paths (`git diff --name-only origin/main...HEAD`)
  into the script.

`shard_size` in `lab/affected.json` sizes the shards: 11 scenarios for `vm`
(each boots a VM, about 40 s apiece on a hosted runner), 60 for the others. A
full run is 11 shards: `vm` 6, `tty` 2, and `wayland`, `x11` and `gnome` 1
each.

A pull request from a fork gets a read-only token and cannot push images. Its
`images` job fails only when an image it needs is missing from GHCR. The GHCR
packages must be readable by the repository's workflows.

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
(`rdkbd`, `lab/rdkbd.py`). The `gnome` environment verifies bindings that
mutter owns: the close binding (default `<Alt>F4`, or rebound to
`<Super>F9`) closes the foot probe, and unbound keys reach it. The
`gnome_shell` environment runs GNOME Shell on the same headless mutter. GNOME
Shell aborts without systemd-logind, so `lab/envs/_logind.py` (a helper, not
an environment) serves a minimal `org.freedesktop.login1` on the system bus;
the shell's state is read back through `org.gnome.Shell.Eval`
(`--unsafe-mode`), and scenarios can start gnome-settings-daemon's
`gsd-media-keys`.

The KDE scenarios run a bare kglobalacceld on Xvfb. For the inactive
component, the "fired" truth is `lab/xgrab.py` showing that kglobalacceld
holds no X11 grab on Meta+Y. The `kwin` environment runs `kwin_wayland
--drm` (`LIBSEAT_BACKEND=noop`) in the KVM guest with the kernel keyboard.
KWin handles global shortcuts in-process and needs the
`org.kde.kglobalaccel` bus name, so no kglobalacceld may start before it
(`kwin-kglobalacceld-owns-name` does that on purpose).

The `cinnamon` environment runs `cinnamon --replace` on Xvfb. muffin keeps a
window's own bindings (maximize, close) on the frame around the focused
client rather than on the root window; `lab/xgrab.py --path` checks the root
and the path to the focused window, and the `wait_grab` action waits on it
before input is sent. Cinnamon reads a custom shortcut when `custom-list`
changes, so scenarios write its fields first and `custom-list` last.

The `ime` environment runs Fcitx5 or IBus on the scenario's session bus,
with a foot probe in headless Sway or an xterm XIM client on Xvfb; checks
`{"committed": text}` and `{"typed": text}` read the client's log. An input
method that composes is tolerated as uncertain: whykey cannot see the
composition.

The `remap` environment starts a remapper daemon (keyd, kanata, xremap,
kmonad, or input-remapper) in the Hyprland KVM guest, where it grabs the
kernel keyboard and emits through `/dev/uinput`.

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
| `actions` | `vkbd`, `rdkbd`, `wtype`, `xdotool`, `type`, `sh`, `sleep`, `wait_quiet`, `start_whykey`, `wait_whykey` (wait for the background whykey to exit; later keys reach the restored session), `mark_probe` (`delivered: "probe"` then counts only keys before this point), or one of the environment's own `ACTIONS` |
| `ground_truth` | `fired` (did the layer act), `delivered` (`probe`, or a check), and `reached` (`fired`, or a check such as `{"probe": "any"}`); `delivered` or `reached` is required. `restored` (optional) is a check that must hold after `wait_whykey`, such as `{"probe": "after_mark"}` (the probe got a key after `mark_probe`) |
| `expect` | `"fired"`: the scenario's binding must fire; if it does not, the run is a lab error, never a pass |
| `whykey.args`, `layer`, `env`, `run` | Command line, layer under test, environment, and how to run it: `direct`, `pty` (typed into the scenario's shell), or `background` (`listen`, with `ready`). For a key sequence (several key arguments) the verdict is the last key's (`steps[-1].path`) |
| `whykey.summary`, `detail`, `binding`, `combo`, `stderr` | Substrings or values that must also match, so a right verdict for the wrong binding still fails. `binding` is a substring of the reported layer's primary binding action |
| `whykey.observation` | Judge `listen`'s observation (`delivered` or not) instead of a layer |
| `whykey.accept` | Honest-uncertainty verdicts tolerated when the scenario documents that whykey cannot know more (for example, a shell without `shell-init`). Shown as `PASS~` and counted separately in the run summary; such passes never count as verification. |
| `whykey.confidence` | The report's `assessment.confidence` must equal this value |
| `whykey.via`, `not_via` | An upstream layer (a remapper) that must, or must not, have redirected the key on its way to `layer` |
| `whykey.silent` | The capture backend cannot see this key (native Sway capture of an unbound key): whykey must give no report and print `stderr`, and the key must have reached the session |
| `whykey.suppress` | `"consumed"` or `"pass"`: `listen --suppress` held the key (not fired, not delivered, reached), and whykey must still report this normal-route verdict; `"pass"` needs a `restored` check proving the key reaches the client after whykey exits |

Probe-window scenarios send the same input twice. The first time, the lab's
own probe client has focus. The second time, `whykey listen --window` has
focus. The bind must fire both times, and whykey's observation must match
what the independent client saw.

## Environment plugins

Each `lab/envs/<name>.py` (names starting with `_` excluded) is the
environment `<name>`; `lab/envs/__init__.py` imports them all, and both
`run.py` (host) and `driver.py` (container) use that registry. A new
environment is one new file, its package families, and its entry in the
`environments` list of its group image in `lab/images.json` (the lab fails
to start when an environment is in no image or in two). Plugins are stdlib
only and share `lab/common.py` (`LabError`, `wait_for`, paths). Every hook
gets the driver's `Lab`: `lab.scenario`, `lab.env` (exported to whykey,
actions and probes), `lab.spawn(name, argv, env)`, `lab.run(argv)`,
`lab.ok(argv)`, `lab.ensure_alive()`, `lab.steps`, `lab.settle`, `lab.pty`.

Registered environments: `cinnamon`, `dwl`, `emacs`, `gnome`, `gnome_shell`,
`hyprland`, `ime`, `kwin`, `labwc`, `niri`, `nvim`, `pty`, `remap`, `sway`,
`terminals`, `wayfire`, and `x11`. Wayfire scenarios list the `ipc-rules`
plugin so whykey can read the live options; `wayfire.py` grants the
container `CAP_SYS_PTRACE`, which root needs to read the compositor's
`/proc/<pid>/maps` (whykey's check that a plugin is loaded).

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
