//! Temporary Hyprland compositor key capture via runtime Lua event hooks.

use std::collections::HashSet;
use std::env;
use std::io::{self, Read};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::capture::{CaptureBackend, NativeBackendId, NativeCaptureIo};
pub use crate::capture::{CapturePolicy as HyprlandCapturePolicy, ChordReleaseStatus};
use crate::command;
use crate::key::KeyCombo;
use crate::listen::{CaptureDisposition, CaptureSource, KeyEventType, ModifierState, ObservedKey};
pub fn hyprland_instance_signature() -> Option<String> {
    env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
        .filter(|sig| !sig.is_empty())
        .and_then(|sig| sig.into_string().ok())
}

pub fn socket2_path(signature: &str) -> Option<PathBuf> {
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
    let candidate = runtime_dir
        .join("hypr")
        .join(signature)
        .join(".socket2.sock");
    if candidate.exists() {
        return Some(candidate);
    }
    let tmp_candidate = PathBuf::from(format!("/tmp/hypr/{signature}/.socket2.sock"));
    if tmp_candidate.exists() {
        return Some(tmp_candidate);
    }
    None
}

pub fn connect_socket2(signature: &str) -> io::Result<UnixStream> {
    let path = socket2_path(signature).ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "Hyprland socket2 path not found")
    })?;
    let stream = UnixStream::connect(path)?;
    stream.set_nonblocking(true)?;
    Ok(stream)
}

pub fn generate_token() -> String {
    format!(
        "wk_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    )
}

pub fn query_active_submap() -> Option<String> {
    let mut cmd = Command::new("hyprctl");
    cmd.args(["submap", "-j"]);
    if let Ok(output) = command::output(&mut cmd) {
        if output.status.success() {
            if let Ok(submap) =
                serde_json::from_str::<String>(&String::from_utf8_lossy(&output.stdout))
            {
                if !submap.trim().is_empty() && submap != "__whykey_capture" {
                    return Some(submap);
                }
            }
        }
    }
    let mut cmd = Command::new("hyprctl");
    cmd.arg("submap");
    if let Ok(output) = command::output(&mut cmd) {
        if output.status.success() {
            let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !line.is_empty() && line != "__whykey_capture" {
                return Some(line);
            }
        }
    }
    None
}

pub fn raw_current_submap() -> String {
    let mut cmd = Command::new("hyprctl");
    cmd.arg("submap");
    if let Ok(output) = command::output(&mut cmd) {
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).trim().to_string();
        }
    }
    String::new()
}

fn submap_target_matches(current: &str, target: &str) -> bool {
    if target == "reset" || target == "default" {
        current.is_empty() || current == "default" || current == "reset"
    } else {
        current == target
    }
}

pub fn has_universal_bindings() -> bool {
    let mut cmd = Command::new("hyprctl");
    cmd.args(["binds", "-j"]);
    let Ok(output) = command::output(&mut cmd) else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return false;
    };
    let Some(binds) = val.as_array() else {
        return false;
    };
    binds.iter().any(|b| {
        b.get("submap_universal")
            .is_some_and(|u| u.as_bool() == Some(true) || u.as_str() == Some("true"))
    })
}

pub fn install_lua_code(token: &str, policy: HyprlandCapturePolicy, _saved_submap: &str) -> String {
    match policy {
        HyprlandCapturePolicy::PassThrough => {
            format!(
                r#"_G.__whykey_subs = _G.__whykey_subs or {{}}
_G.__whykey_subs["{token}"] = hl.on("input.keyboard.key", function(kc, ts, state)
    local mask = 0
    if hl.is_key_down("Shift_L") or hl.is_key_down("Shift_R") then mask = mask + 1 end
    if hl.is_key_down("Control_L") or hl.is_key_down("Control_R") then mask = mask + 4 end
    if hl.is_key_down("Alt_L") or hl.is_key_down("Alt_R") then mask = mask + 8 end
    if hl.is_key_down("Hyper_L") or hl.is_key_down("Hyper_R") then mask = mask + 32 end
    if hl.is_key_down("Super_L") or hl.is_key_down("Super_R") then mask = mask + 64 end
    if hl.is_key_down("ISO_Level3_Shift") or hl.is_key_down("Mode_switch") then mask = mask + 128 end
    hl.dispatch(hl.dsp.event(string.format("whykey-probe,{token},%d,%d,%d", kc, state, mask)))
end)
hl.dispatch(hl.dsp.event("whykey-probe,{token},armed"))"#
            )
        }
        HyprlandCapturePolicy::Suppress => {
            format!(
                r#"_G.__whykey_capture = _G.__whykey_capture or {{}}
if _G.__whykey_capture.owner and _G.__whykey_capture.owner ~= "" then
    error("another suppressing listener is already active")
end

local prev = hl.get_current_submap()
_G.__whykey_capture.owner = "{token}"
_G.__whykey_capture.previous_submap = prev

local function dispatch_submap(target)
    local ok, res = pcall(function() return hl.dispatch(hl.dsp.submap(target)) end)
    if not ok then
        return false, tostring(res)
    end
    if (type(res) == "table" and res.ok == false) or res == false then
        local err = type(res) == "table" and (res.error or "dispatch returned not ok") or "dispatch returned false"
        return false, tostring(err)
    end
    local current_ok, current = pcall(function() return hl.get_current_submap() end)
    if not current_ok then
        return false, tostring(current)
    end
    local restored = (target == "reset" or target == "default")
        and (current == "" or current == "default" or current == "reset")
        or current == target
    if not restored then
        return false, "submap did not change to the requested target"
    end
    return true
end

local function do_cleanup(expected_owner)
    local state = _G.__whykey_capture
    if not state or state.owner ~= expected_owner then
        return
    end
    if state.timer then
        pcall(function() state.timer:set_enabled(false) end)
    end
    if state.listener then
        pcall(function() state.listener:remove() end)
        state.listener = nil
    end
    if state.catchall then
        pcall(function() state.catchall:set_enabled(false) end)
    end
    if state.previous_submap ~= nil then
        local p = state.previous_submap
        local target = (p == "" or p == "default") and "reset" or p
        local restored = dispatch_submap(target)
        if not restored then
            if state.timer then
                pcall(function()
                    state.timer:set_timeout(5000)
                    state.timer:set_enabled(true)
                end)
            end
            return
        end
        state.previous_submap = nil
    end
    state.owner = nil
end

_G.__whykey_capture.on_timeout = function()
    do_cleanup("{token}")
end

local ok, err = pcall(function()
    if not _G.__whykey_capture.timer then
        _G.__whykey_capture.timer = hl.timer(function()
            if _G.__whykey_capture and _G.__whykey_capture.on_timeout then
                _G.__whykey_capture.on_timeout()
            end
        end, {{ timeout = 5000, type = "oneshot" }})
    else
        _G.__whykey_capture.timer:set_timeout(5000)
        _G.__whykey_capture.timer:set_enabled(true)
    end

    if not _G.__whykey_capture.catchall then
        hl.define_submap("__whykey_capture", function()
            _G.__whykey_capture.catchall = hl.bind("catchall", hl.dsp.no_op(), {{ ignore_mods = true }})
        end)
    else
        _G.__whykey_capture.catchall:set_enabled(true)
    end

    _G.__whykey_capture.listener = hl.on("input.keyboard.key", function(kc, ts, state)
        local mask = 0
        if hl.is_key_down("Shift_L") or hl.is_key_down("Shift_R") then mask = mask + 1 end
        if hl.is_key_down("Control_L") or hl.is_key_down("Control_R") then mask = mask + 4 end
        if hl.is_key_down("Alt_L") or hl.is_key_down("Alt_R") then mask = mask + 8 end
        if hl.is_key_down("Hyper_L") or hl.is_key_down("Hyper_R") then mask = mask + 32 end
        if hl.is_key_down("Super_L") or hl.is_key_down("Super_R") then mask = mask + 64 end
        if hl.is_key_down("ISO_Level3_Shift") or hl.is_key_down("Mode_switch") then mask = mask + 128 end
        hl.dispatch(hl.dsp.event(string.format("whykey-probe,{token},%d,%d,%d", kc, state, mask)))
    end)

    local dispatch_result = hl.dispatch(hl.dsp.submap("__whykey_capture"))
    if (type(dispatch_result) == "table" and dispatch_result.ok == false) or dispatch_result == false then
        local err = type(dispatch_result) == "table" and (dispatch_result.error or "dispatch returned not ok") or "dispatch returned false"
        error("capture: failed to activate submap: " .. tostring(err))
    end
    if hl.get_current_submap() ~= "__whykey_capture" then
        error("capture: submap activation was not observed")
    end
    hl.dispatch(hl.dsp.event("whykey-probe,{token},armed"))
end)

if not ok then
    do_cleanup("{token}")
    error(tostring(err))
end"#
            )
        }
    }
}

pub fn cleanup_lua_code(token: &str, policy: HyprlandCapturePolicy, _saved_submap: &str) -> String {
    match policy {
        HyprlandCapturePolicy::PassThrough => {
            format!(
                r#"if _G.__whykey_subs and _G.__whykey_subs["{token}"] then
    pcall(function() _G.__whykey_subs["{token}"]:remove() end)
    _G.__whykey_subs["{token}"] = nil
end"#
            )
        }
        HyprlandCapturePolicy::Suppress => {
            let known_original = !_saved_submap.is_empty() && _saved_submap != "__whykey_capture";
            let default_target = if !known_original || _saved_submap == "default" {
                "reset"
            } else {
                _saved_submap
            };
            // Lua-state-loss recovery runs only with a recorded original:
            // without one, `reset` would be a guess, so refuse instead.
            // The target is baked now: `{recovery}` inserts finished text.
            let recovery = if known_original {
                format!(
                    "    local ok, err = do_dispatch_submap(\"{default_target}\")\n    if not ok then\n        error(\"cleanup: failed to recover submap after Lua state loss: \" .. tostring(err))\n    end"
                )
            } else {
                "    error(\"cleanup: original submap unknown; refusing recovery to a guessed target\")"
                    .to_string()
            };
            format!(
                r#"local state = _G.__whykey_capture
local cur_submap = hl.get_current_submap()
local function do_dispatch_submap(target)
    local ok, res = pcall(function() return hl.dispatch(hl.dsp.submap(target)) end)
    if not ok then
        return false, tostring(res)
    end
    if (type(res) == "table" and res.ok == false) or res == false then
        local err = type(res) == "table" and (res.error or "dispatch returned not ok") or "dispatch returned false"
        return false, tostring(err)
    end
    local current_ok, current = pcall(function() return hl.get_current_submap() end)
    if not current_ok then
        return false, tostring(current)
    end
    local restored = (target == "reset" or target == "default")
        and (current == "" or current == "default" or current == "reset")
        or current == target
    if not restored then
        return false, "submap did not change to the requested target"
    end
    return true
end

if state and state.owner == "{token}" then
    if state.timer then
        pcall(function() state.timer:set_enabled(false) end)
    end
    if state.listener then
        pcall(function() state.listener:remove() end)
        state.listener = nil
    end
    if state.catchall then
        pcall(function() state.catchall:set_enabled(false) end)
    end
    if state.previous_submap ~= nil then
        local p = state.previous_submap
        local target = (p == "" or p == "default") and "reset" or p
        if target == "__whykey_capture" then target = "reset" end
        local ok, err = do_dispatch_submap(target)
        if not ok then
            error("cleanup: failed to restore submap: " .. tostring(err))
        end
        state.previous_submap = nil
    elseif cur_submap == "__whykey_capture" then
        local ok, err = do_dispatch_submap("{default_target}")
        if not ok then
            error("cleanup: failed to restore submap: " .. tostring(err))
        end
    end
    state.owner = nil
elseif state == nil and cur_submap == "__whykey_capture" then
    -- A config reload can clear Lua globals while leaving the compositor in
    -- the private submap. The Rust guard is still the owner of this cleanup
    -- attempt; only recover when the observable state is exactly that private
    -- submap, and restore the submap captured before arming.
{recovery}
elseif state == nil or state.owner == nil or state.owner == "" then
    error("cleanup: not capture owner")
else
    error("cleanup: not capture owner")
end"#
            )
        }
    }
}

pub fn renew_lua_code(token: &str) -> String {
    format!(
        r#"local state = _G.__whykey_capture
if not (state and state.owner == "{token}") then
    error("lease not owned by {token}")
end
if state.timer then
    state.timer:set_timeout(5000)
    state.timer:set_enabled(true)
end"#
    )
}

/// Explicit capture lifecycle. One [`HyprlandCaptureSession`] owns the
/// state; [`HyprlandHookGuard`] only carries the hook identity (token,
/// policy, recorded submap) and the IPC handle used to act on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureState {
    /// Socket connected and original submap recorded; no hook installed.
    Connected,
    /// Install script sent; neither dispatcher success, the submap
    /// postcondition, nor the token-specific armed event is confirmed yet.
    Installing,
    /// Dispatcher succeeded, the private submap is observed, and the armed
    /// event for this token arrived.
    Armed,
    /// Teardown requested; the restored submap is not yet verified.
    RestorePending,
    /// The recorded submap was observed restored. Terminal.
    Closed,
}

/// Narrow Hyprland IPC handle: the only way capture code reaches `hyprctl`.
/// Unit tests inject [`FakeHyprctl`]; production uses [`Hyprctl::Real`].
#[derive(Debug, Clone)]
pub(crate) enum Hyprctl {
    Real,
    #[cfg(test)]
    Fake(std::rc::Rc<FakeHyprctl>),
}

impl Hyprctl {
    pub(crate) fn eval(&self, code: &str) -> io::Result<String> {
        match self {
            Self::Real => {
                let mut command = Command::new("hyprctl");
                command.args(["eval", code]);
                let output =
                    command::output(&mut command).map_err(|e| io::Error::other(e.to_string()))?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if output.status.success() && stdout.contains("ok") {
                    Ok(stdout)
                } else {
                    let detail = if !stderr.trim().is_empty() {
                        stderr.trim().to_string()
                    } else {
                        stdout.trim().to_string()
                    };
                    Err(io::Error::other(detail))
                }
            }
            #[cfg(test)]
            Self::Fake(fake) => {
                fake.eval_calls.borrow_mut().push(code.to_string());
                match fake.eval_results.borrow_mut().pop_front() {
                    Some(result) => result,
                    None => Ok("ok".into()),
                }
            }
        }
    }

    pub(crate) fn current_submap(&self) -> String {
        match self {
            Self::Real => raw_current_submap(),
            #[cfg(test)]
            Self::Fake(fake) => fake.current_submap.borrow().clone(),
        }
    }

    pub(crate) fn devices_json(&self) -> Option<String> {
        match self {
            Self::Real => {
                let mut command = Command::new("hyprctl");
                command.args(["devices", "-j"]);
                let output = command::output(&mut command).ok()?;
                output
                    .status
                    .success()
                    .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
            }
            #[cfg(test)]
            Self::Fake(fake) => fake.devices_json.borrow().clone(),
        }
    }
}

/// Scripted IPC double for capture lifecycle tests. No daemon, no threads:
/// `eval_results` answers successive eval calls, `eval_calls` records the
/// Lua sent, and `current_submap` is the observable compositor state used
/// for postcondition checks.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FakeHyprctl {
    pub eval_results: std::cell::RefCell<std::collections::VecDeque<io::Result<String>>>,
    pub current_submap: std::cell::RefCell<String>,
    pub devices_json: std::cell::RefCell<Option<String>>,
    pub eval_calls: std::cell::RefCell<Vec<String>>,
}

#[cfg(test)]
impl FakeHyprctl {
    pub(crate) fn with_submap(submap: &str) -> std::rc::Rc<Self> {
        std::rc::Rc::new(Self {
            current_submap: std::cell::RefCell::new(submap.to_string()),
            ..Self::default()
        })
    }
}

pub struct HyprlandHookGuard {
    token: String,
    policy: HyprlandCapturePolicy,
    saved_submap: String,
    pub(crate) client: Hyprctl,
}

impl HyprlandHookGuard {
    pub fn new_installed(
        token: String,
        policy: HyprlandCapturePolicy,
        saved_submap: String,
    ) -> Self {
        Self {
            token,
            policy,
            saved_submap,
            client: Hyprctl::Real,
        }
    }

    pub fn new_uninstalled(token: String, saved_submap: String) -> Self {
        Self {
            token,
            policy: HyprlandCapturePolicy::Suppress,
            saved_submap,
            client: Hyprctl::Real,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_client(mut self, client: Hyprctl) -> Self {
        self.client = client;
        self
    }

    pub fn set_policy(&mut self, policy: HyprlandCapturePolicy) {
        self.policy = policy;
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn policy(&self) -> HyprlandCapturePolicy {
        self.policy
    }

    /// The submap recorded before arming. `None` when it was never known,
    /// which forbids Lua-state-loss recovery to a guessed target.
    pub fn saved_submap(&self) -> Option<&str> {
        let saved = self.saved_submap.trim();
        if saved.is_empty() || saved == "__whykey_capture" {
            None
        } else {
            Some(&self.saved_submap)
        }
    }

    /// Owner-checked restore target: the recorded submap, or `reset` for the
    /// default one. Unknown originals have no safe target.
    pub fn restore_target(&self) -> Option<String> {
        let saved = self.saved_submap()?;
        if saved == "default" {
            Some("reset".into())
        } else {
            Some(saved.to_string())
        }
    }

    pub fn renew_lease(&self) -> io::Result<()> {
        if self.policy != HyprlandCapturePolicy::Suppress {
            return Ok(());
        }
        let code = renew_lua_code(&self.token);
        self.client.eval(&code).map_err(|error| {
            io::Error::other(format!(
                "hyprctl eval lease renewal returned error: {error}"
            ))
        })?;
        Ok(())
    }
    /// One owner-checked restore attempt through the Lua route. Does not
    /// clear hook ownership; the session drives retries and the Closed state.
    pub fn restore_submap(&mut self) -> io::Result<()> {
        if self.policy != HyprlandCapturePolicy::Suppress {
            return Ok(());
        }
        let Some(target) = self.restore_target() else {
            return Err(io::Error::other(
                "restore_submap: original submap unknown; refusing to guess a target",
            ));
        };
        let lua = format!(
            r#"local state = _G.__whykey_capture
if state and state.owner and state.owner ~= "{token}" then
    error("restore_submap: capture owned by another session")
end
local res = hl.dispatch(hl.dsp.submap("{target}"))
if (type(res) == "table" and res.ok == false) or res == false then
    local msg = type(res) == "table" and (res.error or "dispatch returned not ok") or "dispatch returned false"
    error("restore_submap: dispatch failed: " .. tostring(msg))
end
local current = hl.get_current_submap()
local restored = ("{target}" == "reset" or "{target}" == "default")
    and (current == "" or current == "default" or current == "reset")
    or current == "{target}"
if not restored then
    error("restore_submap: submap did not change to the requested target")
end"#,
            token = self.token,
            target = target
        );
        if let Err(error) = self.client.eval(&lua) {
            return Err(io::Error::other(format!(
                "hyprctl eval restore submap failed: {error}"
            )));
        }
        // The Lua route already verified the postcondition, but a stale token
        // must never restore a newer capture through the raw dispatch path,
        // so there is no fallback here: surface the error instead.
        Ok(())
    }

    /// One owner-checked teardown attempt through the Lua route. The session
    /// retries and only marks Closed after observing the restored submap.
    pub fn remove(&mut self) -> io::Result<()> {
        let code = cleanup_lua_code(&self.token, self.policy, &self.saved_submap);
        self.client.eval(&code).map(|_| ()).map_err(|error| {
            // Do not dispatch a fallback submap transition after a failed Lua
            // cleanup. The external IPC path can observe only the submap name,
            // not the owner token. A stale guard must never restore a newer
            // capture; the Lua timer remains the owner-checked recovery path
            // when the state exists.
            io::Error::other(format!("hyprctl eval cleanup returned error: {error}"))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketMessage {
    Armed(String),
    Key(HyprlandKeyEvent),
    ActiveLayout,
    ConfigReloaded,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyprlandKeyEvent {
    pub token: String,
    pub xkb_keycode: crate::xkb::XkbKeycode,
    pub event_type: KeyEventType,
    pub modifier_mask: u32,
}

pub fn parse_socket_line(line: &str) -> SocketMessage {
    let trimmed = line.trim();
    if trimmed.starts_with("configreloaded>>") {
        return SocketMessage::ConfigReloaded;
    }
    if trimmed.starts_with("activelayout>>") {
        return SocketMessage::ActiveLayout;
    }
    let Some(payload) = trimmed.strip_prefix("custom>>whykey-probe,") else {
        return SocketMessage::Other;
    };
    let parts: Vec<&str> = payload.split(',').collect();
    if parts.is_empty() {
        return SocketMessage::Other;
    }
    let token = parts[0].to_string();
    if parts.get(1) == Some(&"armed") || parts.get(1) == Some(&"ready") {
        return SocketMessage::Armed(token);
    }
    if parts.len() == 4 {
        let Ok(raw_keycode) = parts[1].parse::<u32>() else {
            return SocketMessage::Other;
        };
        // Socket wire values are XKB keycodes; they enter typed here.
        let xkb_keycode = crate::xkb::XkbKeycode::from(raw_keycode);
        let Ok(state_raw) = parts[2].parse::<u32>() else {
            return SocketMessage::Other;
        };
        let Ok(modifier_mask) = parts[3].parse::<u32>() else {
            return SocketMessage::Other;
        };
        let event_type = match state_raw {
            0 => KeyEventType::Release,
            1 => KeyEventType::Press,
            2 => KeyEventType::Repeat,
            _ => return SocketMessage::Other,
        };
        return SocketMessage::Key(HyprlandKeyEvent {
            token,
            xkb_keycode,
            event_type,
            modifier_mask,
        });
    }
    SocketMessage::Other
}

pub struct HyprlandCaptureSession {
    stream: UnixStream,
    guard: HyprlandHookGuard,
    policy: HyprlandCapturePolicy,
    saved_submap: String,
    keymap: Option<String>,
    group: usize,
    locked_caps: bool,
    locked_num: bool,
    keyboard_state_dirty: bool,
    layout_uncertain: bool,
    buffer: String,
    held_keys: HashSet<crate::xkb::XkbKeycode>,
    current_modifier_mask: u32,
    state: CaptureState,
}

impl HyprlandCaptureSession {
    /// Check whether a Hyprland capture socket is advertised without opening
    /// it or querying compositor state.
    pub fn probe_available() -> bool {
        hyprland_instance_signature()
            .and_then(|signature| socket2_path(&signature))
            .is_some()
    }

    pub fn connect() -> Result<Self, String> {
        let signature =
            hyprland_instance_signature().ok_or("HYPRLAND_INSTANCE_SIGNATURE is not set")?;
        let stream = connect_socket2(&signature)
            .map_err(|e| format!("failed to connect to Hyprland socket2: {e}"))?;

        let saved_submap =
            query_active_submap().ok_or("could not determine the active Hyprland submap safely")?;
        let token = generate_token();
        let guard = HyprlandHookGuard::new_uninstalled(token, saved_submap.clone());

        let mut session = Self {
            stream,
            guard,
            policy: HyprlandCapturePolicy::Suppress,
            saved_submap,
            keymap: None,
            group: 0,
            locked_caps: false,
            locked_num: false,
            keyboard_state_dirty: false,
            layout_uncertain: false,
            buffer: String::new(),
            held_keys: HashSet::new(),
            current_modifier_mask: 0,
            state: CaptureState::Connected,
        };

        session.refresh_keyboard_state();
        Ok(session)
    }

    pub fn arm(&mut self, policy: HyprlandCapturePolicy) -> Result<(), String> {
        if self.state != CaptureState::Connected && self.state != CaptureState::Closed {
            return Err(format!(
                "cannot arm Hyprland capture from state {:?}; close it first",
                self.state
            ));
        }
        self.policy = policy;
        self.guard.set_policy(policy);
        self.state = CaptureState::Installing;

        let token = self.guard.token().to_string();
        let lua = install_lua_code(&token, policy, &self.saved_submap);

        // Dispatcher success: the install script raises when `hl.dispatch`
        // returns false, `{ ok = false }`, or leaves the submap unchanged.
        if let Err(error) = self.guard.client.eval(&lua) {
            return Err(self.fail_arm(format!("hyprctl eval failed: {error}")));
        }
        // Suppression uses a private submap, so its activation is an explicit
        // postcondition. Pass-through deliberately leaves the active submap
        // unchanged and only needs the token-specific armed event.
        if policy == HyprlandCapturePolicy::Suppress
            && self.guard.client.current_submap() != "__whykey_capture"
        {
            return Err(self.fail_arm("capture: submap activation was not observed".into()));
        }

        let deadline = Instant::now() + Duration::from_millis(1500);
        let mut armed_received = false;
        while Instant::now() < deadline {
            if let Err(error) = self.read_incoming() {
                return Err(self.fail_arm(format!(
                    "failed while waiting for Hyprland hook armed confirmation: {error}"
                )));
            }
            while let Some(pos) = self.buffer.find('\n') {
                let line = self.buffer[..pos].to_string();
                self.buffer = self.buffer[pos + 1..].to_string();
                if let SocketMessage::Armed(armed_token) = &parse_socket_line(&line) {
                    if armed_token == &token {
                        armed_received = true;
                        break;
                    }
                }
            }
            if armed_received {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        if !armed_received {
            return Err(
                self.fail_arm("timed out waiting for Hyprland hook armed confirmation".into())
            );
        }

        self.state = CaptureState::Armed;
        Ok(())
    }

    /// Leave Installing for RestorePending and run the bounded teardown so a
    /// failed arm never reports an armed hook. Returns the original error
    /// with the cleanup outcome appended.
    fn fail_arm(&mut self, error: String) -> String {
        match self.cleanup_bounded() {
            Ok(()) => format!("{error}; capture cleaned up"),
            Err(cleanup) => format!("{error}; cleanup also failed: {cleanup}"),
        }
    }

    pub fn open() -> Result<Self, String> {
        let mut session = Self::connect()?;
        session.arm(HyprlandCapturePolicy::Suppress)?;
        Ok(session)
    }

    pub fn socket_fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    pub fn policy(&self) -> HyprlandCapturePolicy {
        self.policy
    }

    pub fn state(&self) -> CaptureState {
        self.state
    }

    pub fn is_suppressing(&self) -> bool {
        self.policy == HyprlandCapturePolicy::Suppress && self.state == CaptureState::Armed
    }

    pub fn saved_submap(&self) -> &str {
        &self.saved_submap
    }

    pub fn restore_submap(&mut self) -> io::Result<()> {
        self.cleanup_bounded()
    }

    pub fn renew_lease(&self) -> io::Result<()> {
        self.guard.renew_lease()
    }

    pub fn close(&mut self) -> io::Result<()> {
        self.cleanup_bounded()
    }

    /// Bounded teardown through the owner-checked Lua route: enter
    /// RestorePending, retry the attempt twice, and reach Closed only after
    /// observing the recorded submap for suppression. Pass-through only owns
    /// its listener, so it closes after the owner-checked removal succeeds.
    /// A permanent failure stays RestorePending so Drop can warn instead of
    /// pretending success.
    fn cleanup_bounded(&mut self) -> io::Result<()> {
        if self.state == CaptureState::Closed {
            return Ok(());
        }
        if self.state == CaptureState::Connected {
            // No hook was ever installed; nothing to restore.
            self.state = CaptureState::Closed;
            return Ok(());
        }
        self.state = CaptureState::RestorePending;
        if self.policy == HyprlandCapturePolicy::Suppress && self.guard.restore_target().is_none() {
            // Never guess a restore target: without the recorded original,
            // the Lua route would fall back to `reset`, which may not be the
            // session's submap. Stay RestorePending and say so.
            return Err(io::Error::other(
                "original submap unknown; refusing to guess a restore target \
                 (run `hyprctl dispatch submap reset` manually if the private \
                 `__whykey_capture` submap is still active)",
            ));
        }
        let mut last_error = String::new();
        for attempt in 0..3 {
            match self.guard.remove() {
                Ok(()) if self.policy == HyprlandCapturePolicy::PassThrough => {
                    self.state = CaptureState::Closed;
                    return Ok(());
                }
                Ok(()) if self.verify_restored() => {
                    self.state = CaptureState::Closed;
                    return Ok(());
                }
                Ok(()) => {
                    last_error = format!(
                        "cleanup reported success but the submap was not restored (still {:?})",
                        self.guard.client.current_submap()
                    );
                }
                Err(error) => last_error = error.to_string(),
            }
            if attempt < 2 {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Err(io::Error::other(last_error))
    }

    /// Confirm the compositor shows the recorded submap after teardown.
    fn verify_restored(&self) -> bool {
        let current = self.guard.client.current_submap();
        match self.guard.restore_target() {
            None => false,
            Some(target) => submap_target_matches(&current, &target),
        }
    }

    pub fn refresh_keyboard_state(&mut self) {
        self.keyboard_state_dirty = false;
        if let Some(devices_json) = self.guard.client.devices_json() {
            self.keymap = crate::layers::hyprland::compile_main_xkb_keymap(&devices_json);
            self.group = crate::layers::hyprland::main_keyboard_active_layout_index(&devices_json)
                .unwrap_or(0);
            let (caps, num) = crate::layers::hyprland::main_keyboard_lock_state(&devices_json);
            self.locked_caps = caps;
            self.locked_num = num;
            self.layout_uncertain = false;
            return;
        }
        self.layout_uncertain = true;
    }

    pub fn read_incoming(&mut self) -> io::Result<usize> {
        let mut chunk = [0u8; 4096];
        match self.stream.read(&mut chunk) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "socket closed",
            )),
            Ok(n) => {
                self.buffer.push_str(&String::from_utf8_lossy(&chunk[..n]));
                Ok(n)
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(e),
        }
    }

    pub fn next_observed_event(&mut self, events_all: bool) -> Result<Option<ObservedKey>, String> {
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 1..].to_string();
            match parse_socket_line(&line) {
                SocketMessage::ConfigReloaded => {
                    return Err(
                        "Hyprland configuration reloaded while listening; capture stopped".into(),
                    );
                }
                SocketMessage::ActiveLayout => {
                    self.keyboard_state_dirty = true;
                }
                SocketMessage::Key(event) if event.token == self.guard.token() => {
                    // The socket speaks XKB; cross to evdev once via the
                    // single conversion. Values below the offset have no
                    // evdev counterpart and behave as key zero downstream.
                    let evdev_code = event.xkb_keycode.to_evdev().map_or(0, |code| code.get());

                    self.current_modifier_mask = event.modifier_mask;
                    match event.event_type {
                        KeyEventType::Press => {
                            self.held_keys.insert(event.xkb_keycode);
                        }
                        KeyEventType::Release => {
                            self.held_keys.remove(&event.xkb_keycode);
                        }
                        _ => {}
                    }

                    if matches!(evdev_code, 58 | 69) {
                        self.keyboard_state_dirty = true;
                    }

                    if self.keyboard_state_dirty {
                        self.refresh_keyboard_state();
                    }

                    let observed = self.decode_event(&event);

                    if !events_all {
                        if observed.event_type != KeyEventType::Press {
                            continue;
                        }
                        if matches!(evdev_code, 58 | 69)
                            || crate::listen::evdev_modifier(evdev_code).is_some()
                        {
                            continue;
                        }
                    }

                    return Ok(Some(observed));
                }
                _ => {}
            }
        }
        Ok(None)
    }
    pub fn wait_for_chord_release(
        &mut self,
        main_keycode: crate::xkb::XkbKeycode,
        max_duration: Duration,
    ) -> Result<ChordReleaseStatus, String> {
        let deadline = Instant::now() + max_duration;
        let mut last_renew = Instant::now();

        self.drain_buffer_events()?;
        if self.chord_release_confirmed(main_keycode) {
            return Ok(ChordReleaseStatus::Released);
        }

        while Instant::now() < deadline {
            if self.policy == HyprlandCapturePolicy::Suppress
                && last_renew.elapsed() >= Duration::from_secs(2)
            {
                self.renew_lease().map_err(|e| {
                    format!("failed to renew capture lease during chord release: {e}")
                })?;
                last_renew = Instant::now();
            }

            if self.chord_release_confirmed(main_keycode) {
                return Ok(ChordReleaseStatus::Released);
            }

            let mut pollfd = libc::pollfd {
                fd: self.stream.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout_ms = (remaining.as_millis() as i32).clamp(1, 50);
            let ret = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
            if ret == -1 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(format!("poll error during chord release: {err}"));
            }
            if pollfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                return Err("Hyprland socket disconnected during chord release".into());
            }
            if ret > 0 && (pollfd.revents & libc::POLLIN != 0) {
                self.read_incoming().map_err(|e| {
                    format!("failed to read Hyprland socket during chord release: {e}")
                })?;
                self.drain_buffer_events()?;
            }
        }
        if self.keyboard_state_dirty {
            self.refresh_keyboard_state();
        }
        Ok(ChordReleaseStatus::TimedOut)
    }

    fn chord_release_confirmed(&self, main_keycode: crate::xkb::XkbKeycode) -> bool {
        !self.held_keys.contains(&main_keycode)
            && self.held_keys.is_empty()
            && self.current_modifier_mask == 0
    }

    fn drain_buffer_events(&mut self) -> Result<(), String> {
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 1..].to_string();
            match parse_socket_line(&line) {
                SocketMessage::ConfigReloaded => {
                    return Err(
                        "Hyprland configuration reloaded while listening; capture stopped".into(),
                    );
                }
                SocketMessage::ActiveLayout => {
                    self.keyboard_state_dirty = true;
                }
                SocketMessage::Key(event) if event.token == self.guard.token() => {
                    self.current_modifier_mask = event.modifier_mask;
                    match event.event_type {
                        KeyEventType::Press => {
                            self.held_keys.insert(event.xkb_keycode);
                        }
                        KeyEventType::Release => {
                            self.held_keys.remove(&event.xkb_keycode);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn decode_event(&self, event: &HyprlandKeyEvent) -> ObservedKey {
        // The socket speaks XKB; cross to evdev once via the single
        // conversion. Values below the offset have no evdev counterpart.
        let evdev_code = event.xkb_keycode.to_evdev();
        let evdev_raw = evdev_code.map_or(0, |code| code.get());

        // 1. Adjust modifier state according to press or release of the current modifier
        let mut effective_mask = event.modifier_mask;
        if let Some(mod_bit) = crate::listen::evdev_modifier(evdev_raw) {
            match event.event_type {
                KeyEventType::Press | KeyEventType::Repeat => {
                    effective_mask |= mod_bit;
                }
                KeyEventType::Release => {
                    effective_mask &= !mod_bit;
                }
            }
        }

        // 2. Base unshifted symbol (level 0) for Hyprland binding matching.
        // The event already carries the XKB keycode, so one lookup suffices;
        // the old evdev/raw dual lookup could only disagree on namespace.
        let base_key_name = self
            .keymap
            .as_deref()
            .and_then(|km| {
                crate::xkb::preferred_symbol_for_xkb_keycode(km, event.xkb_keycode, self.group, 0)
            })
            .or_else(|| crate::listen::evdev_key_name(evdev_raw).map(str::to_owned))
            .unwrap_or_else(|| format!("CODE:{evdev_raw}"));

        // 3. Normalize aliases at this boundary
        let normalized_base = crate::key::normalize_key_str(&base_key_name);

        let combo_mods = match crate::listen::evdev_modifier(evdev_raw) {
            Some(own_mod) => effective_mask & !own_mod,
            None => effective_mask,
        };
        let combo = KeyCombo::from_parts(combo_mods, &normalized_base);

        // 4. Shifted / AltGr level symbol for alternate display evidence
        let shift = effective_mask & 1 != 0;
        let altgr = effective_mask & 128 != 0;
        let xkb_level = if altgr {
            usize::from(shift) + 2
        } else {
            usize::from(shift)
        };
        let shifted_keysym = if xkb_level > 0 {
            self.keymap
                .as_deref()
                .and_then(|km| {
                    crate::xkb::preferred_symbol_for_xkb_keycode(
                        km,
                        event.xkb_keycode,
                        self.group,
                        xkb_level,
                    )
                })
                .filter(|sym| crate::key::normalize_key_str(sym) != normalized_base)
        } else {
            None
        };

        // 5. Pressed modifiers for ModifierState
        let mut pressed_mods = Vec::new();
        if effective_mask & 4 != 0 {
            pressed_mods.push("CTRL".into());
        }
        if effective_mask & 8 != 0 {
            pressed_mods.push("ALT".into());
        }
        if effective_mask & 1 != 0 {
            pressed_mods.push("SHIFT".into());
        }
        if effective_mask & 64 != 0 {
            pressed_mods.push("SUPER".into());
        }
        if effective_mask & 32 != 0 {
            pressed_mods.push("HYPER".into());
        }
        if effective_mask & 128 != 0 {
            pressed_mods.push("ALTGR".into());
        }

        let mut locked = Vec::new();
        if self.locked_caps {
            locked.push("CAPSLOCK".into());
        }
        if self.locked_num {
            locked.push("NUMLOCK".into());
        }

        let modifier_state = ModifierState {
            pressed: pressed_mods,
            locked,
            latched: None,
            devices: Vec::new(),
        };

        let encoding = if self.layout_uncertain {
            "Hyprland XKB key event (layout interpretation uncertain)".into()
        } else {
            "Hyprland XKB key event".into()
        };

        let alternate_key = match shifted_keysym {
            Some(sym) => Some(format!(
                "physical keycode {evdev_raw} ({normalized_base}); shifted keysym: {sym}"
            )),
            None => Some(format!("physical keycode {evdev_raw} ({normalized_base})")),
        };

        let disposition = match self.policy {
            HyprlandCapturePolicy::Suppress => CaptureDisposition::Suppressed,
            HyprlandCapturePolicy::PassThrough => CaptureDisposition::PassedThrough,
        };

        ObservedKey {
            combo,
            raw: Vec::new(),
            raw_display: Some(format!(
                "Hyprland XKB keycode={} evdev={} ({})",
                event.xkb_keycode.get(),
                evdev_raw,
                event.event_type.label()
            )),
            modifier_state: Some(modifier_state),
            associated_text: None,
            physical_keycode: evdev_code,
            encoding,
            protocol_flags: None,
            event_type: event.event_type,
            alternate_keys: None,
            alternate_key,
            source: CaptureSource::CompositorNative {
                backend: "Hyprland".into(),
            },
            disposition,
        }
    }
}

impl CaptureBackend for HyprlandCaptureSession {
    fn id(&self) -> NativeBackendId {
        NativeBackendId::Hyprland
    }

    fn display(&self) -> &'static str {
        NativeBackendId::Hyprland.display()
    }

    fn arm(&mut self, policy: crate::capture::CapturePolicy) -> Result<(), String> {
        Self::arm(self, policy)
    }

    fn next_observed_event(&mut self, events_all: bool) -> Result<Option<ObservedKey>, String> {
        Self::next_observed_event(self, events_all)
    }

    fn wait_for_chord_release(
        &mut self,
        main: crate::xkb::XkbKeycode,
        timeout: Duration,
    ) -> Result<ChordReleaseStatus, String> {
        Self::wait_for_chord_release(self, main, timeout)
    }

    fn is_suppressing(&self) -> bool {
        Self::is_suppressing(self)
    }

    fn close(&mut self) -> io::Result<()> {
        Self::close(self)
    }
}

impl NativeCaptureIo for HyprlandCaptureSession {
    fn socket_fd(&self) -> RawFd {
        Self::socket_fd(self)
    }

    fn read_incoming(&mut self) -> io::Result<usize> {
        Self::read_incoming(self)
    }

    fn renew_lease(&self) -> io::Result<()> {
        Self::renew_lease(self)
    }
}

impl Drop for HyprlandCaptureSession {
    fn drop(&mut self) {
        if self.state == CaptureState::Closed || self.state == CaptureState::Connected {
            return;
        }
        // One final best-effort attempt: no retries, no sleep. A stale token
        // still cannot restore another session because the Lua route checks
        // ownership; warn loudly instead of failing silently.
        let outcome = self
            .guard
            .remove()
            .map_err(|error| error.to_string())
            .and_then(|()| {
                if self.policy == HyprlandCapturePolicy::PassThrough || self.verify_restored() {
                    Ok(())
                } else {
                    Err(format!(
                        "submap was not observed restored (still {:?})",
                        self.guard.client.current_submap()
                    ))
                }
            });
        match outcome {
            Ok(()) => self.state = CaptureState::Closed,
            Err(error) => {
                self.state = CaptureState::RestorePending;
                eprintln!("warning: failed to clean up Hyprland key hook: {error}");
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn decode_test_event(
    event: &HyprlandKeyEvent,
    keymap: Option<&str>,
    group: usize,
    locked_caps: bool,
    locked_num: bool,
) -> ObservedKey {
    let session = HyprlandCaptureSession {
        stream: UnixStream::pair().unwrap().0,
        guard: HyprlandHookGuard::new_uninstalled(event.token.clone(), "default".into()),
        policy: HyprlandCapturePolicy::Suppress,
        saved_submap: "default".into(),
        keymap: keymap.map(str::to_owned),
        group,
        locked_caps,
        locked_num,
        keyboard_state_dirty: false,
        layout_uncertain: false,
        buffer: String::new(),
        held_keys: HashSet::new(),
        current_modifier_mask: 0,
        state: CaptureState::Closed,
    };
    session.decode_event(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_session_satisfies_capture_backend_trait() {
        fn assert_backend<T: crate::capture::CaptureBackend>() {}
        assert_backend::<HyprlandCaptureSession>();
    }

    #[test]
    fn parse_complete_fragmented_and_combined_socket_records() {
        // Complete record
        let line = "custom>>whykey-probe,tok1,36,1,68";
        let parsed = parse_socket_line(line);
        assert_eq!(
            parsed,
            SocketMessage::Key(HyprlandKeyEvent {
                token: "tok1".into(),
                xkb_keycode: crate::xkb::XkbKeycode::from(36),
                event_type: KeyEventType::Press,
                modifier_mask: 68,
            })
        );

        // Fragmented lines across reads
        let chunk1 = "custom>>whykey-probe,tok1,36";
        let chunk2 = ",0,0\n";
        let mut buffer = String::new();
        buffer.push_str(chunk1);
        assert_eq!(buffer.find('\n'), None);
        buffer.push_str(chunk2);
        let pos = buffer.find('\n').unwrap();
        let line = buffer[..pos].to_string();
        buffer = buffer[pos + 1..].to_string();
        assert_eq!(
            parse_socket_line(&line),
            SocketMessage::Key(HyprlandKeyEvent {
                token: "tok1".into(),
                xkb_keycode: crate::xkb::XkbKeycode::from(36),
                event_type: KeyEventType::Release,
                modifier_mask: 0,
            })
        );
        assert!(buffer.is_empty());

        // Combined records in one buffer
        let combined = "custom>>whykey-probe,tok1,armed\ncustom>>whykey-probe,tok1,36,2,4\n";
        let mut lines = Vec::new();
        let mut rem = combined;
        while let Some(pos) = rem.find('\n') {
            lines.push(rem[..pos].to_string());
            rem = &rem[pos + 1..];
        }
        assert_eq!(lines.len(), 2);
        assert_eq!(
            parse_socket_line(&lines[0]),
            SocketMessage::Armed("tok1".into())
        );
        assert_eq!(
            parse_socket_line(&lines[1]),
            SocketMessage::Key(HyprlandKeyEvent {
                token: "tok1".into(),
                xkb_keycode: crate::xkb::XkbKeycode::from(36),
                event_type: KeyEventType::Repeat,
                modifier_mask: 4,
            })
        );
    }

    #[test]
    fn ignore_unrelated_hyprland_events_and_events_from_another_token() {
        assert_eq!(
            parse_socket_line("activelayout>>keyboard,us"),
            SocketMessage::ActiveLayout
        );
        assert_eq!(
            parse_socket_line("openwindow>>0x123,workspace,class,title"),
            SocketMessage::Other
        );
        assert_eq!(
            parse_socket_line("custom>>other-event,data"),
            SocketMessage::Other
        );

        let parsed = parse_socket_line("custom>>whykey-probe,other_token,36,1,0");
        match parsed {
            SocketMessage::Key(event) => {
                assert_ne!(event.token, "my_token");
            }
            _ => panic!("expected key event"),
        }
    }

    #[test]
    fn decode_press_repeat_and_release_correctly() {
        let press = parse_socket_line("custom>>whykey-probe,tok,36,1,0");
        let repeat = parse_socket_line("custom>>whykey-probe,tok,36,2,0");
        let release = parse_socket_line("custom>>whykey-probe,tok,36,0,0");

        assert!(matches!(press, SocketMessage::Key(e) if e.event_type == KeyEventType::Press));
        assert!(matches!(repeat, SocketMessage::Key(e) if e.event_type == KeyEventType::Repeat));
        assert!(matches!(release, SocketMessage::Key(e) if e.event_type == KeyEventType::Release));
    }

    #[test]
    fn convert_xkb_keycode_36_to_evdev_28_and_return() {
        let event = HyprlandKeyEvent {
            token: "tok".into(),
            xkb_keycode: crate::xkb::XkbKeycode::from(36),
            event_type: KeyEventType::Press,
            modifier_mask: 0,
        };
        let observed = decode_test_event(&event, None, 0, false, false);
        assert_eq!(
            observed.physical_keycode,
            Some(crate::xkb::EvdevKeycode::from(28))
        );
        assert_eq!(observed.combo.key(), "RETURN");
        assert_eq!(observed.combo.modmask(), 0);
        assert_eq!(
            observed.source,
            CaptureSource::CompositorNative {
                backend: "Hyprland".into()
            }
        );
        assert_eq!(observed.encoding, "Hyprland XKB key event");
        assert_eq!(observed.disposition, CaptureDisposition::Suppressed);
    }

    #[test]
    fn preserve_ctrl_super_modifier_state() {
        let event = HyprlandKeyEvent {
            token: "tok".into(),
            xkb_keycode: crate::xkb::XkbKeycode::from(36),
            event_type: KeyEventType::Press,
            modifier_mask: 4 | 64, // CTRL (4) | SUPER (64)
        };
        let observed = decode_test_event(&event, None, 0, false, false);
        assert_eq!(observed.combo.key(), "RETURN");
        assert_eq!(observed.combo.modmask(), 4 | 64);
        assert_eq!(observed.combo.to_string(), "CTRL + SUPER + RETURN");
        let state = observed.modifier_state.unwrap();
        assert!(state.pressed.contains(&"CTRL".to_string()));
        assert!(state.pressed.contains(&"SUPER".to_string()));
    }

    #[test]
    fn modifier_event_ordering_press_and_release() {
        let press_event = HyprlandKeyEvent {
            token: "tok".into(),
            xkb_keycode: crate::xkb::XkbKeycode::from(37),
            event_type: KeyEventType::Press,
            modifier_mask: 0,
        };
        let observed_press = decode_test_event(&press_event, None, 0, false, false);
        assert_eq!(observed_press.combo.key(), "LEFTCTRL");
        assert_eq!(observed_press.combo.modmask(), 0);
        let press_state = observed_press.modifier_state.unwrap();
        assert!(
            press_state.pressed.contains(&"CTRL".to_string()),
            "press must adjust modifier state to include pressed modifier"
        );

        let release_event = HyprlandKeyEvent {
            token: "tok".into(),
            xkb_keycode: crate::xkb::XkbKeycode::from(37),
            event_type: KeyEventType::Release,
            modifier_mask: 4,
        };
        let observed_release = decode_test_event(&release_event, None, 0, false, false);
        assert_eq!(observed_release.combo.key(), "LEFTCTRL");
        let release_state = observed_release.modifier_state.unwrap();
        assert!(
            !release_state.pressed.contains(&"CTRL".to_string()),
            "release must adjust modifier state to clear released modifier"
        );
    }

    #[test]
    fn shifted_symbol_hyprland_binding_matching() {
        let keymap = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AE01> = 10;
};
xkb_symbols "pc" {
    key <AE01> {
        symbols[1] = [ 1, exclam ]
    };
};
};
"#;
        let event = HyprlandKeyEvent {
            token: "tok".into(),
            xkb_keycode: crate::xkb::XkbKeycode::from(10),
            event_type: KeyEventType::Press,
            modifier_mask: 1, // SHIFT
        };
        let observed = decode_test_event(&event, Some(keymap), 0, false, false);
        assert_eq!(
            observed.combo.key(),
            "1",
            "base symbol must be used for combo key"
        );
        assert_eq!(observed.combo.modmask(), 1);
        assert_eq!(observed.combo.to_string(), "SHIFT + 1");
        assert_eq!(
            observed.associated_text, None,
            "associated_text must remain empty for Hyprland"
        );
        assert!(
            observed
                .alternate_key
                .as_deref()
                .unwrap()
                .contains("shifted keysym: exclam"),
            "alternate evidence must preserve shifted keysym"
        );
    }

    #[test]
    fn lua_code_contains_consuming_modifier_independent_catchall() {
        let code = install_lua_code("test_token", HyprlandCapturePolicy::Suppress, "default");
        assert!(code.contains("hl.define_submap(\"__whykey_capture\""));
        assert!(code.contains("hl.bind(\"catchall\", hl.dsp.no_op(), { ignore_mods = true })"));
        assert!(code.contains("_G.__whykey_capture.catchall:set_enabled(true)"));
    }

    #[test]
    fn original_submap_saved_and_restored() {
        let code = install_lua_code("test_token", HyprlandCapturePolicy::Suppress, "default");
        assert!(code.contains("local prev = hl.get_current_submap()"));
        assert!(code.contains("_G.__whykey_capture.previous_submap = prev"));

        let cleanup = cleanup_lua_code("test_token", HyprlandCapturePolicy::Suppress, "default");
        assert!(cleanup.contains("hl.dispatch(hl.dsp.submap(target))"));
        assert!(cleanup.contains("(p == \"\" or p == \"default\") and \"reset\" or p"));
    }

    #[test]
    fn submap_target_matching_accepts_reset_as_default_only() {
        assert!(submap_target_matches("default", "reset"));
        assert!(submap_target_matches("reset", "default"));
        assert!(submap_target_matches("resize", "resize"));
        assert!(!submap_target_matches("default", "resize"));
        assert!(!submap_target_matches("__whykey_capture", "reset"));
    }

    #[test]
    fn generated_suppression_lua_is_syntactically_valid() {
        let Some(luac) = std::env::var_os("WHYKEY_LUAC").or_else(|| {
            Command::new("sh")
                .args(["-c", "command -v luac"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|path| path.trim().to_owned())
                .filter(|path| !path.is_empty())
                .map(std::ffi::OsString::from)
        }) else {
            return;
        };
        let path = std::env::temp_dir().join(format!(
            "whykey-capture-{}-{}.lua",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(
            &path,
            install_lua_code("test_token", HyprlandCapturePolicy::Suppress, "default"),
        )
        .unwrap();
        let result = Command::new(luac).args(["-p"]).arg(&path).status();
        let _ = std::fs::remove_file(&path);
        assert!(result.expect("luac must run").success());
    }

    #[test]
    fn armed_event_emitted_only_after_submap_activation() {
        let code = install_lua_code("test_token", HyprlandCapturePolicy::Suppress, "default");
        let submap_pos = code
            .find("local dispatch_result = hl.dispatch(hl.dsp.submap(\"__whykey_capture\"))")
            .expect("submap switch");
        let armed_pos = code
            .find("hl.dispatch(hl.dsp.event(\"whykey-probe,test_token,armed\"))")
            .expect("armed event");
        assert!(
            submap_pos < armed_pos,
            "submap activation must precede armed event emission"
        );
        assert!(code.contains("dispatch_result.ok == false"));
        assert!(code.contains("hl.get_current_submap() ~= \"__whykey_capture\""));
        assert!(code.contains(
            "current_ok, current = pcall(function() return hl.get_current_submap() end)"
        ));
    }

    #[test]
    fn stale_tokens_cannot_disarm_another_session() {
        let cleanup = cleanup_lua_code("token_a", HyprlandCapturePolicy::Suppress, "default");
        assert!(cleanup.contains("if state and state.owner == \"token_a\" then"));
    }

    #[test]
    fn catchall_cleanup_disables_rather_than_removes_it() {
        let cleanup = cleanup_lua_code("test_token", HyprlandCapturePolicy::Suppress, "default");
        assert!(cleanup.contains("state.catchall:set_enabled(false)"));
        assert!(!cleanup.contains("catchall:remove()"));
        assert!(!cleanup.contains("catchall:unbind()"));
        assert!(!cleanup.contains("bind:remove()"));
    }

    #[test]
    fn second_suppressing_listener_raises_lua_error() {
        let code = install_lua_code("test_token", HyprlandCapturePolicy::Suppress, "default");
        assert!(
            code.contains(
                "if _G.__whykey_capture.owner and _G.__whykey_capture.owner ~= \"\" then"
            )
        );
        assert!(code.contains("error(\"another suppressing listener is already active\")"));
    }

    #[test]
    fn failsafe_timer_included_in_lua_setup() {
        let code = install_lua_code("test_token", HyprlandCapturePolicy::Suppress, "default");
        assert!(code.contains("timeout = 5000, type = \"oneshot\""));
        assert!(code.contains("do_cleanup(\"test_token\")"));
    }

    #[test]
    fn capture_policy_defaults_to_suppress() {
        use crate::listen::Options;
        let default_opts = Options::default();
        assert_eq!(default_opts.capture_policy, HyprlandCapturePolicy::Suppress);
    }

    #[test]
    fn renew_lua_code_checks_ownership_and_errors() {
        let renew = renew_lua_code("test_token");
        assert!(renew.contains("if not (state and state.owner == \"test_token\") then"));
        assert!(renew.contains("error(\"lease not owned by test_token\")"));
    }

    #[test]
    fn cleanup_lua_code_errors_when_not_owner() {
        let cleanup = cleanup_lua_code("token_a", HyprlandCapturePolicy::Suppress, "default");
        assert!(cleanup.contains("error(\"cleanup: not capture owner\")"));
    }

    #[test]
    fn cleanup_lua_code_recovers_private_submap_after_state_loss() {
        let cleanup = cleanup_lua_code("token_a", HyprlandCapturePolicy::Suppress, "resize");
        assert!(cleanup.contains("state == nil and cur_submap == \"__whykey_capture\""));
        assert!(cleanup.contains("failed to recover submap after Lua state loss"));
        assert!(cleanup.contains("do_dispatch_submap(\"resize\")"));
    }

    #[test]
    fn chord_release_waiting_does_not_discard_config_reload() {
        let (s1, _s2) = std::os::unix::net::UnixStream::pair().unwrap();
        s1.set_nonblocking(true).unwrap();
        let guard = HyprlandHookGuard::new_uninstalled("tok".into(), "default".into());
        let mut session = HyprlandCaptureSession {
            stream: s1,
            guard,
            policy: HyprlandCapturePolicy::Suppress,
            saved_submap: "default".into(),
            keymap: None,
            group: 0,
            locked_caps: false,
            locked_num: false,
            keyboard_state_dirty: false,
            layout_uncertain: false,
            buffer: "configreloaded>>\n".into(),
            held_keys: HashSet::new(),
            current_modifier_mask: 0,
            state: CaptureState::Closed,
        };
        let res = session.drain_buffer_events();
        assert!(res.is_err(), "config reload must return error");
        assert!(res.unwrap_err().contains("configuration reloaded"));
    }

    #[test]
    fn chord_release_waiting_marks_layout_dirty_on_active_layout() {
        let (s1, _s2) = std::os::unix::net::UnixStream::pair().unwrap();
        s1.set_nonblocking(true).unwrap();
        let guard = HyprlandHookGuard::new_uninstalled("tok".into(), "default".into());
        let mut session = HyprlandCaptureSession {
            stream: s1,
            guard,
            policy: HyprlandCapturePolicy::Suppress,
            saved_submap: "default".into(),
            keymap: None,
            group: 0,
            locked_caps: false,
            locked_num: false,
            keyboard_state_dirty: false,
            layout_uncertain: false,
            buffer: "activelayout>>keyboard,English\n".into(),
            held_keys: HashSet::new(),
            current_modifier_mask: 0,
            state: CaptureState::Closed,
        };
        let res = session.drain_buffer_events();
        assert!(res.is_ok());
        assert!(
            session.keyboard_state_dirty,
            "active layout must mark keyboard state dirty"
        );
    }

    #[test]
    fn cleanup_lua_code_refuses_to_restore_when_owner_state_is_missing() {
        let cleanup = cleanup_lua_code("test_token", HyprlandCapturePolicy::Suppress, "default");
        let missing_owner = cleanup
            .find("elseif state == nil or state.owner == nil or state.owner == \"\" then")
            .unwrap();
        let dispatch = cleanup.find("do_dispatch_submap(\"reset\")").unwrap();
        assert!(dispatch < missing_owner);
        assert!(cleanup[missing_owner..].contains("error(\"cleanup: not capture owner\")"));
    }

    #[test]
    fn chord_release_waiting_returns_err_on_poll_hangup() {
        let (s1, s2) = std::os::unix::net::UnixStream::pair().unwrap();
        s1.set_nonblocking(true).unwrap();
        // Drop s2 so poll detects POLLHUP
        drop(s2);

        let guard = HyprlandHookGuard::new_uninstalled("tok".into(), "default".into());
        let mut session = HyprlandCaptureSession {
            stream: s1,
            guard,
            policy: HyprlandCapturePolicy::Suppress,
            saved_submap: "default".into(),
            keymap: None,
            group: 0,
            locked_caps: false,
            locked_num: false,
            keyboard_state_dirty: false,
            layout_uncertain: false,
            buffer: String::new(),
            held_keys: [crate::xkb::XkbKeycode::from(10)].into_iter().collect(),
            current_modifier_mask: 0,
            state: CaptureState::Closed,
        };
        let res = session
            .wait_for_chord_release(crate::xkb::XkbKeycode::from(10), Duration::from_millis(100));
        assert!(res.is_err(), "socket disconnect must return error");
        assert!(res.unwrap_err().contains("disconnected"));
    }

    #[test]
    fn chord_release_waiting_reports_timeout_without_confirmed_release() {
        let (s1, _s2) = std::os::unix::net::UnixStream::pair().unwrap();
        s1.set_nonblocking(true).unwrap();
        let guard = HyprlandHookGuard::new_uninstalled("tok".into(), "default".into());
        let mut session = HyprlandCaptureSession {
            stream: s1,
            guard,
            policy: HyprlandCapturePolicy::Suppress,
            saved_submap: "default".into(),
            keymap: None,
            group: 0,
            locked_caps: false,
            locked_num: false,
            keyboard_state_dirty: false,
            layout_uncertain: false,
            buffer: String::new(),
            held_keys: [crate::xkb::XkbKeycode::from(10)].into_iter().collect(),
            current_modifier_mask: 4,
            state: CaptureState::Closed,
        };

        assert_eq!(
            session
                .wait_for_chord_release(crate::xkb::XkbKeycode::from(10), Duration::ZERO)
                .unwrap(),
            ChordReleaseStatus::TimedOut
        );
    }
    /// Build a session whose IPC answers come from the scripted fake. The
    /// peer end is returned so the socket stays connected; drop it to
    /// simulate transport loss.
    fn fake_session(
        saved_submap: &str,
        state: CaptureState,
        fake: std::rc::Rc<FakeHyprctl>,
    ) -> (std::os::unix::net::UnixStream, HyprlandCaptureSession) {
        let (s1, s2) = std::os::unix::net::UnixStream::pair().unwrap();
        s1.set_nonblocking(true).unwrap();
        let guard = HyprlandHookGuard::new_uninstalled("tok".into(), saved_submap.into())
            .with_client(Hyprctl::Fake(fake));
        let session = HyprlandCaptureSession {
            stream: s1,
            guard,
            policy: HyprlandCapturePolicy::Suppress,
            saved_submap: saved_submap.into(),
            keymap: None,
            group: 0,
            locked_caps: false,
            locked_num: false,
            keyboard_state_dirty: false,
            layout_uncertain: false,
            buffer: String::new(),
            held_keys: HashSet::new(),
            current_modifier_mask: 0,
            state,
        };
        (s2, session)
    }

    fn eval_err(message: &str) -> io::Result<String> {
        Err(io::Error::other(message))
    }

    #[test]
    fn arm_rejects_a_false_dispatcher_result() {
        let fake = FakeHyprctl::with_submap("default");
        fake.eval_results
            .borrow_mut()
            .push_back(eval_err("dispatch returned false"));
        let (_peer, mut session) = fake_session("default", CaptureState::Connected, fake.clone());
        let result = session.arm(HyprlandCapturePolicy::Suppress);
        assert!(result.is_err(), "a false dispatcher must not arm");
        assert!(
            result.unwrap_err().contains("dispatch returned false"),
            "the dispatcher evidence must reach the error"
        );
        assert_ne!(session.state(), CaptureState::Armed);
    }

    #[test]
    fn arm_rejects_an_error_table_dispatcher_result() {
        let fake = FakeHyprctl::with_submap("default");
        fake.eval_results
            .borrow_mut()
            .push_back(eval_err("dispatch returned not ok"));
        let (_peer, mut session) = fake_session("default", CaptureState::Connected, fake.clone());
        let result = session.arm(HyprlandCapturePolicy::Suppress);
        assert!(result.is_err(), "an error table must not arm");
        assert_ne!(session.state(), CaptureState::Armed);
    }

    #[test]
    fn arm_rejects_success_without_an_observed_submap_change() {
        // The install eval claims success, but the compositor never entered
        // the private submap: the owner-side postcondition must refuse Armed.
        let fake = FakeHyprctl::with_submap("default");
        let (_peer, mut session) = fake_session("default", CaptureState::Connected, fake.clone());
        let result = session.arm(HyprlandCapturePolicy::Suppress);
        assert!(result.is_err(), "an unobserved submap must not arm");
        assert!(
            result.unwrap_err().contains("not observed"),
            "the postcondition failure must be named"
        );
        assert_ne!(session.state(), CaptureState::Armed);
    }

    #[test]
    fn arm_reaches_armed_only_after_postcondition_and_armed_event() {
        let fake = FakeHyprctl::with_submap("__whykey_capture");
        let (_peer, mut session) = fake_session("default", CaptureState::Connected, fake.clone());
        session.buffer.push_str("custom>>whykey-probe,tok,armed\n");
        session
            .arm(HyprlandCapturePolicy::Suppress)
            .expect("postcondition plus armed event must arm");
        assert_eq!(session.state(), CaptureState::Armed);
        assert!(session.is_suppressing());
    }

    #[test]
    fn pass_through_arm_succeeds_without_private_submap_activation() {
        let fake = FakeHyprctl::with_submap("default");
        let (_peer, mut session) = fake_session("default", CaptureState::Connected, fake);
        session.buffer.push_str("custom>>whykey-probe,tok,armed\n");
        session
            .arm(HyprlandCapturePolicy::PassThrough)
            .expect("pass-through must not require the suppression submap");
        assert_eq!(session.state(), CaptureState::Armed);
        assert!(!session.is_suppressing());
    }

    #[test]
    fn pass_through_cleanup_does_not_restore_a_user_changed_submap() {
        let fake = FakeHyprctl::with_submap("default");
        let (_peer, mut session) = fake_session("default", CaptureState::Connected, fake.clone());
        session.buffer.push_str("custom>>whykey-probe,tok,armed\n");
        session
            .arm(HyprlandCapturePolicy::PassThrough)
            .expect("pass-through must arm");

        *fake.current_submap.borrow_mut() = "resize".into();
        session.close().expect("pass-through cleanup must close");

        assert_eq!(session.state(), CaptureState::Closed);
        assert_eq!(fake.current_submap.borrow().as_str(), "resize");
        assert_eq!(fake.eval_calls.borrow().len(), 2);
        assert!(
            !fake.eval_calls.borrow()[1].contains("hl.dsp.submap"),
            "pass-through cleanup must not dispatch submap restoration"
        );
    }

    #[test]
    fn pass_through_drop_removes_only_its_listener() {
        let fake = FakeHyprctl::with_submap("default");
        let (_peer, mut session) = fake_session("default", CaptureState::Connected, fake.clone());
        session.buffer.push_str("custom>>whykey-probe,tok,armed\n");
        session
            .arm(HyprlandCapturePolicy::PassThrough)
            .expect("pass-through must arm");

        *fake.current_submap.borrow_mut() = "resize".into();
        drop(session);

        assert_eq!(fake.current_submap.borrow().as_str(), "resize");
        assert_eq!(fake.eval_calls.borrow().len(), 2);
        assert!(
            !fake.eval_calls.borrow()[1].contains("hl.dsp.submap"),
            "pass-through drop must not dispatch submap restoration"
        );
    }

    #[test]
    fn arm_reports_socket_loss_while_installing() {
        let fake = FakeHyprctl::with_submap("__whykey_capture");
        let (peer, mut session) = fake_session("default", CaptureState::Connected, fake.clone());
        drop(peer);
        let result = session.arm(HyprlandCapturePolicy::Suppress);
        assert!(result.is_err(), "socket loss must not arm");
        assert!(
            result.unwrap_err().contains("armed confirmation"),
            "the failure must name the missing armed event"
        );
        assert_ne!(session.state(), CaptureState::Armed);
    }

    #[test]
    fn cleanup_retries_once_failing_attempt_then_closes() {
        let fake = FakeHyprctl::with_submap("default");
        fake.eval_results.borrow_mut().push_back(eval_err("boom"));
        fake.eval_results.borrow_mut().push_back(Ok("ok".into()));
        let (_peer, mut session) = fake_session("default", CaptureState::Armed, fake.clone());
        session.close().expect("second attempt must close");
        assert_eq!(session.state(), CaptureState::Closed);
        assert_eq!(fake.eval_calls.borrow().len(), 2);
    }

    #[test]
    fn permanent_cleanup_failure_stays_restore_pending() {
        let fake = FakeHyprctl::with_submap("__whykey_capture");
        for _ in 0..3 {
            fake.eval_results.borrow_mut().push_back(eval_err("boom"));
        }
        let (_peer, mut session) = fake_session("default", CaptureState::Armed, fake.clone());
        let result = session.close();
        assert!(result.is_err(), "permanent failure must surface");
        assert_eq!(session.state(), CaptureState::RestorePending);
        assert_eq!(
            fake.eval_calls.borrow().len(),
            3,
            "cleanup retries twice through the owner-checked route"
        );
    }

    #[test]
    fn close_after_reload_restores_and_closes() {
        let fake = FakeHyprctl::with_submap("default");
        let (_peer, mut session) = fake_session("default", CaptureState::Armed, fake.clone());
        session.buffer.push_str("configreloaded>>\n");
        let observed = session.next_observed_event(true);
        assert!(observed.is_err(), "a reload must stop capture loudly");
        session.close().expect("post-reload cleanup must close");
        assert_eq!(session.state(), CaptureState::Closed);
    }

    #[test]
    fn close_restores_a_custom_original_submap() {
        let fake = FakeHyprctl::with_submap("gaming");
        let (_peer, mut session) = fake_session("gaming", CaptureState::Armed, fake.clone());
        session.close().expect("custom submap must restore");
        assert_eq!(session.state(), CaptureState::Closed);
        assert!(
            fake.eval_calls.borrow()[0].contains("gaming"),
            "cleanup must target the recorded custom submap"
        );
    }

    #[test]
    fn close_with_unknown_original_refuses_to_guess() {
        let fake = FakeHyprctl::with_submap("__whykey_capture");
        let (_peer, mut session) = fake_session("", CaptureState::Armed, fake.clone());
        let result = session.close();
        assert!(result.is_err(), "an unknown original must not restore");
        assert!(
            result.unwrap_err().to_string().contains("unknown"),
            "the refusal must name the missing original"
        );
        assert_eq!(session.state(), CaptureState::RestorePending);
        assert!(
            fake.eval_calls.borrow().is_empty(),
            "no Lua may run without a known target"
        );
    }

    #[test]
    fn stale_token_never_restores_another_session() {
        let fake = FakeHyprctl::with_submap("__whykey_capture");
        for _ in 0..3 {
            fake.eval_results
                .borrow_mut()
                .push_back(eval_err("cleanup: not capture owner"));
        }
        let (_peer, mut session) = fake_session("default", CaptureState::Armed, fake.clone());
        let result = session.close();
        assert!(result.is_err(), "a stale token must not close");
        assert_eq!(session.state(), CaptureState::RestorePending);
        assert_eq!(
            fake.eval_calls.borrow().len(),
            3,
            "every retry stays on the owner-checked Lua route"
        );
        assert!(
            fake.eval_calls.borrow()[0].contains("tok"),
            "retries present the stale token for the owner check"
        );
    }

    #[test]
    fn lua_state_loss_recovery_needs_a_known_original() {
        let known = cleanup_lua_code("tok", HyprlandCapturePolicy::Suppress, "gaming");
        assert!(known.contains("do_dispatch_submap(\"gaming\")"));
        let unknown = cleanup_lua_code("tok", HyprlandCapturePolicy::Suppress, "");
        let stateless = unknown
            .find("state == nil and cur_submap == \"__whykey_capture\"")
            .expect("the stateless recovery branch must exist");
        assert!(
            unknown[stateless..].contains("original submap unknown; refusing recovery"),
            "recovery without a recorded original must refuse"
        );
    }
}
