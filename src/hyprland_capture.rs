//! Temporary Hyprland compositor key capture via runtime Lua event hooks.

use std::collections::HashSet;
use std::env;
use std::io::{self, Read};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::command;
use crate::key::KeyCombo;
use crate::listen::{CaptureDisposition, CaptureSource, KeyEventType, ModifierState, ObservedKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HyprlandCapturePolicy {
    #[default]
    Suppress,
    PassThrough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordReleaseStatus {
    Released,
    TimedOut,
}
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

#[cfg(test)]
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
            let default_target = if _saved_submap.is_empty()
                || _saved_submap == "default"
                || _saved_submap == "__whykey_capture"
            {
                "reset"
            } else {
                _saved_submap
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
    local ok, err = do_dispatch_submap("{default_target}")
    if not ok then
        error("cleanup: failed to recover submap after Lua state loss: " .. tostring(err))
    end
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

pub struct HyprlandHookGuard {
    token: String,
    policy: HyprlandCapturePolicy,
    saved_submap: String,
    installed: bool,
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
            installed: true,
        }
    }

    pub fn new_uninstalled(token: String, saved_submap: String) -> Self {
        Self {
            token,
            policy: HyprlandCapturePolicy::Suppress,
            saved_submap,
            installed: false,
        }
    }

    pub fn is_installed(&self) -> bool {
        self.installed
    }

    pub fn set_installed(&mut self, installed: bool) {
        self.installed = installed;
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

    pub fn renew_lease(&self) -> io::Result<()> {
        if self.policy != HyprlandCapturePolicy::Suppress || !self.installed {
            return Ok(());
        }
        let code = renew_lua_code(&self.token);
        let mut command = Command::new("hyprctl");
        command.args(["eval", &code]);
        let output = command::output(&mut command).map_err(|e| io::Error::other(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status.success() && stdout.contains("ok") {
            Ok(())
        } else {
            let err = if !output.stderr.is_empty() {
                String::from_utf8_lossy(&output.stderr).to_string()
            } else {
                stdout.to_string()
            };
            Err(io::Error::other(format!(
                "hyprctl eval lease renewal returned error: {}",
                err.trim()
            )))
        }
    }

    pub fn restore_submap(&mut self) -> io::Result<()> {
        if self.policy != HyprlandCapturePolicy::Suppress {
            return Ok(());
        }
        let target = if self.saved_submap.is_empty()
            || self.saved_submap == "default"
            || self.saved_submap == "__whykey_capture"
        {
            "reset"
        } else {
            &self.saved_submap
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
        let mut cmd = Command::new("hyprctl");
        cmd.args(["eval", &lua]);
        if let Ok(output) = command::output(&mut cmd) {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if output.status.success() && stdout.contains("ok") {
                return Ok(());
            } else {
                let err = if !output.stderr.is_empty() {
                    String::from_utf8_lossy(&output.stderr).to_string()
                } else {
                    stdout.to_string()
                };
                return Err(io::Error::other(format!(
                    "hyprctl eval restore submap failed: {}",
                    err.trim()
                )));
            }
        }
        let mut submap_cmd = Command::new("hyprctl");
        submap_cmd.args(["dispatch", "submap", target]);
        let output =
            command::output(&mut submap_cmd).map_err(|e| io::Error::other(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status.success() && (stdout.trim() == "ok" || stdout.contains("ok")) {
            Ok(())
        } else {
            let err = if !stdout.trim().is_empty() && stdout.trim() != "ok" {
                stdout.trim().to_string()
            } else {
                String::from_utf8_lossy(&output.stderr).to_string()
            };
            Err(io::Error::other(format!(
                "hyprctl dispatch submap returned error: {}",
                err.trim()
            )))
        }
    }

    pub fn remove(&mut self) -> io::Result<()> {
        if !self.installed {
            return Ok(());
        }
        let code = cleanup_lua_code(&self.token, self.policy, &self.saved_submap);
        let mut command = Command::new("hyprctl");
        command.args(["eval", &code]);
        let output = command::output(&mut command).map_err(|e| io::Error::other(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let eval_ok = output.status.success() && stdout.contains("ok");

        // Do not dispatch a fallback submap transition after a failed Lua
        // cleanup. The external IPC path can observe only the submap name, not
        // the owner token. A stale guard must never restore a newer capture.
        // Keep the guard installed and surface the error instead; the Lua
        // timer remains the owner-checked recovery path when the state exists.
        let cleanup_ok = eval_ok;

        if cleanup_ok {
            self.installed = false;
            Ok(())
        } else {
            let err = if !stderr.is_empty() {
                stderr.to_string()
            } else {
                stdout.to_string()
            };
            Err(io::Error::other(format!(
                "hyprctl eval cleanup returned error: {}",
                err.trim()
            )))
        }
    }
}

impl Drop for HyprlandHookGuard {
    fn drop(&mut self) {
        if self.installed {
            if let Err(err) = self.remove() {
                eprintln!("warning: failed to clean up Hyprland key hook: {err}");
            }
        }
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
    pub xkb_keycode: u32,
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
        let Ok(xkb_keycode) = parts[1].parse::<u32>() else {
            return SocketMessage::Other;
        };
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
    held_keys: HashSet<u32>,
    current_modifier_mask: u32,
    armed: bool,
}

impl HyprlandCaptureSession {
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
            armed: false,
        };

        session.refresh_keyboard_state();
        Ok(session)
    }

    pub fn arm(&mut self, policy: HyprlandCapturePolicy) -> Result<(), String> {
        self.policy = policy;
        self.guard.set_policy(policy);

        let token = self.guard.token().to_string();
        let lua = install_lua_code(&token, policy, &self.saved_submap);

        // Enable cleanup before evaluation so that partial execution or failures trigger cleanup.
        self.guard.set_installed(true);

        let mut command = Command::new("hyprctl");
        command.args(["eval", &lua]);
        let output = command::output(&mut command).map_err(|e| {
            let cleanup = self.guard.remove().err().map_or_else(String::new, |error| {
                format!("; cleanup also failed: {error}")
            });
            format!("failed to execute hyprctl: {e}{cleanup}")
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || !stdout.contains("ok") {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let cleanup = self.guard.remove().err().map_or_else(String::new, |error| {
                format!("; cleanup also failed: {error}")
            });
            let msg = if !stderr.trim().is_empty() {
                stderr.trim()
            } else {
                stdout.trim()
            };
            return Err(format!("hyprctl eval failed: {msg}{cleanup}"));
        }

        let deadline = Instant::now() + Duration::from_millis(1500);
        let mut armed_received = false;
        while Instant::now() < deadline {
            if let Err(error) = self.read_incoming() {
                let cleanup = self.guard.remove().err().map_or_else(String::new, |error| {
                    format!("; cleanup also failed: {error}")
                });
                return Err(format!(
                    "failed while waiting for Hyprland hook armed confirmation: {error}{cleanup}"
                ));
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
            let cleanup = self.guard.remove().err().map_or_else(String::new, |error| {
                format!("; cleanup also failed: {error}")
            });
            return Err(format!(
                "timed out waiting for Hyprland hook armed confirmation{cleanup}"
            ));
        }

        self.armed = true;
        Ok(())
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

    pub fn is_suppressing(&self) -> bool {
        self.policy == HyprlandCapturePolicy::Suppress && self.guard.is_installed()
    }

    pub fn saved_submap(&self) -> &str {
        &self.saved_submap
    }

    pub fn restore_submap(&mut self) -> io::Result<()> {
        self.guard.restore_submap()
    }

    pub fn renew_lease(&self) -> io::Result<()> {
        self.guard.renew_lease()
    }

    pub fn close(&mut self) -> io::Result<()> {
        self.guard.remove()
    }

    pub fn refresh_keyboard_state(&mut self) {
        self.keyboard_state_dirty = false;
        let mut dev_cmd = Command::new("hyprctl");
        dev_cmd.args(["devices", "-j"]);
        if let Ok(dev_output) = command::output(&mut dev_cmd) {
            if dev_output.status.success() {
                let devices_json = String::from_utf8_lossy(&dev_output.stdout);
                self.keymap = crate::layers::hyprland::compile_main_xkb_keymap(&devices_json);
                self.group =
                    crate::layers::hyprland::main_keyboard_active_layout_index(&devices_json)
                        .unwrap_or(0);
                let (caps, num) = crate::layers::hyprland::main_keyboard_lock_state(&devices_json);
                self.locked_caps = caps;
                self.locked_num = num;
                self.layout_uncertain = false;
                return;
            }
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
                    let evdev_code = if event.xkb_keycode >= 8 {
                        (event.xkb_keycode - 8) as u16
                    } else {
                        event.xkb_keycode as u16
                    };

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
        main_keycode: u32,
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

    fn chord_release_confirmed(&self, main_keycode: u32) -> bool {
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
        let evdev_code = if event.xkb_keycode >= 8 {
            (event.xkb_keycode - 8) as u16
        } else {
            event.xkb_keycode as u16
        };

        // 1. Adjust modifier state according to press or release of the current modifier
        let mut effective_mask = event.modifier_mask;
        if let Some(mod_bit) = crate::listen::evdev_modifier(evdev_code) {
            match event.event_type {
                KeyEventType::Press | KeyEventType::Repeat => {
                    effective_mask |= mod_bit;
                }
                KeyEventType::Release => {
                    effective_mask &= !mod_bit;
                }
            }
        }

        // 2. Base unshifted symbol (level 0) for Hyprland binding matching
        let base_key_name = self
            .keymap
            .as_deref()
            .and_then(|km| {
                crate::xkb::preferred_symbol_for_xkb_keycode(km, event.xkb_keycode, self.group, 0)
                    .or_else(|| {
                        crate::xkb::preferred_symbol_for_evdev_keycode(
                            km, evdev_code, self.group, 0,
                        )
                    })
            })
            .or_else(|| crate::listen::evdev_key_name(evdev_code).map(str::to_owned))
            .unwrap_or_else(|| format!("CODE:{evdev_code}"));

        // 3. Normalize aliases at this boundary
        let normalized_base = crate::key::normalize_key_str(&base_key_name);

        let combo_mods = match crate::listen::evdev_modifier(evdev_code) {
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
                    .or_else(|| {
                        crate::xkb::preferred_symbol_for_evdev_keycode(
                            km, evdev_code, self.group, xkb_level,
                        )
                    })
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
                "physical keycode {evdev_code} ({normalized_base}); shifted keysym: {sym}"
            )),
            None => Some(format!("physical keycode {evdev_code} ({normalized_base})")),
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
                event.xkb_keycode,
                evdev_code,
                event.event_type.label()
            )),
            modifier_state: Some(modifier_state),
            associated_text: None,
            physical_keycode: Some(evdev_code),
            encoding,
            protocol_flags: None,
            event_type: event.event_type,
            alternate_keys: None,
            alternate_key,
            source: CaptureSource::Hyprland,
            disposition,
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
        armed: true,
    };
    session.decode_event(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_complete_fragmented_and_combined_socket_records() {
        // Complete record
        let line = "custom>>whykey-probe,tok1,36,1,68";
        let parsed = parse_socket_line(line);
        assert_eq!(
            parsed,
            SocketMessage::Key(HyprlandKeyEvent {
                token: "tok1".into(),
                xkb_keycode: 36,
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
                xkb_keycode: 36,
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
                xkb_keycode: 36,
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
            xkb_keycode: 36,
            event_type: KeyEventType::Press,
            modifier_mask: 0,
        };
        let observed = decode_test_event(&event, None, 0, false, false);
        assert_eq!(observed.physical_keycode, Some(28));
        assert_eq!(observed.combo.key(), "RETURN");
        assert_eq!(observed.combo.modmask(), 0);
        assert_eq!(observed.source, CaptureSource::Hyprland);
        assert_eq!(observed.encoding, "Hyprland XKB key event");
        assert_eq!(observed.disposition, CaptureDisposition::Suppressed);
    }

    #[test]
    fn preserve_ctrl_super_modifier_state() {
        let event = HyprlandKeyEvent {
            token: "tok".into(),
            xkb_keycode: 36,
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
            xkb_keycode: 37,
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
            xkb_keycode: 37,
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
            xkb_keycode: 10,
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
            armed: true,
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
            armed: true,
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
            held_keys: [10].into_iter().collect(),
            current_modifier_mask: 0,
            armed: true,
        };
        let res = session.wait_for_chord_release(10, Duration::from_millis(100));
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
            held_keys: [10].into_iter().collect(),
            current_modifier_mask: 4,
            armed: true,
        };

        assert_eq!(
            session.wait_for_chord_release(10, Duration::ZERO).unwrap(),
            ChordReleaseStatus::TimedOut
        );
    }

}
