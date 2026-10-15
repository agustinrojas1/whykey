# Lab replay

The reality lab (`lab/`) proves whykey's verdicts against real programs, one
Docker container per scenario. That costs minutes per scenario, and a pull
request that only changes whykey's code repeats all of it. The ground truth
of a scenario does not change when whykey changes; the program does, and
the lab catches that. Replay separates the two. A lab run records every
external input whykey read, and a plain `cargo test` feeds those inputs back
into whykey and checks that it still gives the verdict the lab accepted.

## What is recorded

Every question whykey asks its environment goes through `src/recording.rs`:

| Input | Function | Recorded as |
| --- | --- | --- |
| child process (`tmux`, `gsettings`, `swaymsg`, `hyprctl`, ...) | `command_output` (all of `command::output`) | program, args, cwd, env changes, stdin, and the exit status, streams or error |
| environment variable | `env_var`, `env_var_os` | the value, or absent |
| file contents | `read`, `read_to_string`, `read_prefix`, `read_regular_prefix` | the bytes, or the io error |
| directory listing | `read_dir` | each name and type, or the error |
| symbolic link | `read_link`, `canonicalize` | the target |
| metadata and existence | `metadata`, `symlink_metadata`, `PathInputs` | size, type, symlink, or the error |
| process id, parent id | `process_id`, `parent_process_id` | the number |
| controlling terminal | `tty_termios`, `can_open("/dev/tty")` | the termios fields |

Each answer is encoded to JSON and decoded again before whykey sees it, in a
recording run as well as in a replay, so the recorded run already sees what a
replay will show it. Known losses: environment values, paths and names that
are not UTF-8 are recorded lossily, and a directory entry whose type cannot
be read is replayed as an error.

## Modes

- `WHYKEY_RECORD=<file>`: each answer is appended to `<file>` as one JSON
  line, and returned unchanged.
- `WHYKEY_REPLAY=<recording.json>`: each answer comes from the recording, and
  nothing outside the process is read or started.
- Neither set (production): every function is the standard library call it
  wraps. Both set is an error (status 96).

A question the recording holds no answer to ends the replay with status 97
and `unrecorded input: ...` on stderr. An answer the run never asked for
ends it with status 98 (`recorded input never requested`). Neither case
prints a verdict, so a changed query forces a real lab run.

## Recording format

`lab/recordings/<scenario>.json`, one input per line:

- `format`, `scenario`, `file`, `group`, `environment`, `description`,
  `recorded_on`, `image`, `label` (the ground truth the lab saw);
- `whykey`: `version`, `commit`, `argv` (the exact arguments, with
  `--json --schema-version 2`), `run`;
  A pty scenario types its whykey command into the scenario's shell. The variable
  is exported there (`export`, or `set -gx` for fish), not set on an `env` command,
  because shell-init defines `whykey` as a shell function that exports the readline
  snapshot before it runs the binary, and `env` would skip that function.
- `expect`: `verdicts` (the set the lab accepted: `expected` in
  `lab/run.py`), `layers`, and the scenario's own text checks `summary`,
  `detail`, `binding`, `stderr`, `confidence`;
- `inputs`: the ordered list of `{kind, key, response}`.

The verdict rules are the ones in `lab/run.py` (`layer_verdict`, `evaluate`):
`tests/lab_replay.rs` ports the status/propagation mapping and the text
checks, and reads the accepted set from `expect`.

## Refreshing and checking

`python3 lab/run.py --record lab/recordings [--only GLOB]` runs the lab as
usual. For each scenario that passed, it replays the recording on the host,
with an empty environment, through the same `evaluate` rules. The recording
is written only when the replay reproduces the lab's verdict. Otherwise the
scenario is reported as not replayable, with the reason.

`cargo test --test lab_replay` replays every recording and checks the
verdict. A recording that no longer matches is reported by scenario.

## Not replayable

Scenarios are excluded, with the reason in the lab output, when they depend on
live input:

- `run: background` (`listen`): it reads the keyboard, a capture socket, or
  an X11 grab while it runs.
- `observation`, `silent`, `suppress`: they judge a live key.
- `via`, `not_via`: they check an upstream remapper's redirect, which the
  replay check does not port yet.

`tests/lab_replay.rs` also audits the source: `raw_input_calls_go_through_the_recording_module`
fails when a raw call (`env::var`, `fs::read`, `read_dir`, `File::open`,
`UnixStream::connect`, `process::id`, ...) appears outside `src/recording.rs`
and the files in `RAW_INPUT_ALLOWED`. That list names the live-only files
(listen, probes, the capture sockets, the X11 grab) and the one layer input
not recorded yet: the Wayfire IPC socket in `src/layers/wayfire.rs`.

## Risks

- A recording goes stale when the program version changes (the image or its
  packages). The next lab run reports it as a mismatch, and the recording is
  refreshed with `--record`. The recording records the image id so a refresh
  can be attributed.
- A replay is only as complete as the hooks. An input read through a raw
  call would be read from the test machine. The static check guards the
  common cases, not every path.
- Replay is faster than the lab, so timing-based budgets (`with_deadline`)
  never expire in a replay. Recorded command outcomes include budget
  exhaustion, and the verdict is checked, but a loop that polls a deadline
  without a recorded command is not reproduced.
