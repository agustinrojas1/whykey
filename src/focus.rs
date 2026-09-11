use crate::environment::Environment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusedTarget {
    pub pid: u32,
    pub source: &'static str,
}

pub fn resolve() -> Result<FocusedTarget, String> {
    let environment = Environment::collect();
    resolve_with_environment(&environment)
}

pub fn resolve_with_environment(environment: &Environment) -> Result<FocusedTarget, String> {
    if let Some(id) = environment.selected_compositor {
        if let Some(entry) = crate::registry::DESKTOPS
            .iter()
            .find(|entry| entry.id == id)
        {
            if let Some((source, focused_pid)) = entry.focused_pid.or(entry.focus) {
                if let Ok(pid) = focused_pid() {
                    return Ok(FocusedTarget { pid, source });
                }
            }
        }
    }
    if let Some(adapter) = environment.extension_adapters.iter().find(|adapter| {
        crate::extension_adapters::applicable_with_context(adapter, &environment.compositor_context)
            && adapter.focused_cmd.is_some()
    }) {
        return crate::extension_adapters::focused_pid(adapter).map(|pid| FocusedTarget {
            pid,
            source: "compositor extension",
        });
    }
    if environment.selected_compositor.is_none() {
        return Err(
            "no supported compositor session was detected for focused-window discovery".into(),
        );
    }
    Err("the detected desktop does not expose a read-only focused-window PID API".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_target_keeps_the_evidence_source() {
        let target = FocusedTarget {
            pid: 42,
            source: "fixture",
        };
        assert_eq!(target.pid, 42);
        assert_eq!(target.source, "fixture");
    }

    #[test]
    fn focus_selection_follows_registry_priority() {
        let mut env = Environment::collect();
        env.selected_compositor = Some("sway");
        let target = crate::registry::DESKTOPS
            .iter()
            .find(|entry| entry.id == env.selected_compositor.unwrap())
            .unwrap();
        assert_eq!(target.id, "sway");

        let hypr_pos = crate::registry::DESKTOPS
            .iter()
            .position(|e| e.id == "hyprland")
            .unwrap();
        let sway_pos = crate::registry::DESKTOPS
            .iter()
            .position(|e| e.id == "sway")
            .unwrap();
        let i3_pos = crate::registry::DESKTOPS
            .iter()
            .position(|e| e.id == "i3")
            .unwrap();
        assert!(hypr_pos < sway_pos);
        assert!(sway_pos < i3_pos);
    }

    #[test]
    fn selected_desktop_without_focus_returns_unsupported_message() {
        let mut env = Environment::collect();
        env.selected_compositor = Some("kde");
        assert_eq!(
            resolve_with_environment(&env),
            Err("the detected desktop does not expose a read-only focused-window PID API".into())
        );

        env.selected_compositor = Some("gnome");
        assert_eq!(
            resolve_with_environment(&env),
            Err("the detected desktop does not expose a read-only focused-window PID API".into())
        );

        env.selected_compositor = Some("compositor");
        assert_eq!(
            resolve_with_environment(&env),
            Err("the detected desktop does not expose a read-only focused-window PID API".into())
        );
    }

    #[test]
    fn no_selected_compositor_returns_not_detected_message() {
        let mut env = Environment::collect();
        env.selected_compositor = None;
        assert_eq!(
            resolve_with_environment(&env),
            Err("no supported compositor session was detected for focused-window discovery".into())
        );
    }
}
