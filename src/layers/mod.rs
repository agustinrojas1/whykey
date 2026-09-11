pub mod application;
pub mod cinnamon;
pub mod compositor;
pub mod ghostty;
pub mod gnome;
pub mod hyprland;
pub mod i3;
pub mod inspection;
pub mod kde;
pub mod labwc;
pub mod mate;
pub mod model;
pub mod niri;
pub mod openbox;
pub mod programmable;
pub mod readline;
pub mod river;
pub mod screen;
pub mod session;
pub mod shell;
pub mod sway;
pub mod sxhkd;
pub mod terminal;
pub mod terminal_app;
pub mod terminal_dispatch;
pub mod tmux;
pub mod wayfire;
pub mod x11;
pub mod xfce;
pub mod zellij;

pub use inspection::*;
pub use model::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command;
    use crate::key::KeyCombo;

    #[test]
    fn formats_bytes_consistently() {
        assert_eq!(format_bytes(&[0x1b, b'[', b'1', 0x1a]), "ESC [ 1 0x1a");
    }

    #[test]
    fn keeps_session_after_the_terminal() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let layers = inspect_default_chain(&key);
        let names: Vec<_> = layers.iter().map(|layer| layer.layer).collect();
        if let Some(session_index) = names.iter().position(|name| *name == "Session context") {
            assert!(names[..session_index].contains(&"TTY driver"));
        }
    }

    #[test]
    fn reports_an_exhausted_static_diagnostic_budget() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let layers =
            command::with_deadline(std::time::Duration::ZERO, || inspect_default_chain(&key));

        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, "Diagnostic budget");
        assert_eq!(layers[0].status(), LayerStatus::Unavailable);
    }

    #[test]
    fn outcome_round_trips_through_serialized_fields() {
        for outcome in [
            Outcome::Pass,
            Outcome::HandledAndPassed,
            Outcome::Consumed,
            Outcome::Redirected,
            Outcome::HandledUncertain,
            Outcome::UncertainContinues,
            Outcome::Unknown,
            Outcome::Unavailable,
        ] {
            let (status, propagation) = outcome.as_parts();
            assert_eq!(Outcome::from_parts(&status, &propagation), outcome);
        }
    }

    #[test]
    fn layer_ids_cover_typed_route_order() {
        assert!(ROUTE_ORDER.contains(&LayerId::Tty));
        assert!(ROUTE_ORDER.contains(&LayerId::Ime));
        let tty = ROUTE_ORDER
            .iter()
            .position(|id| *id == LayerId::Tty)
            .unwrap();
        let ime = ROUTE_ORDER
            .iter()
            .position(|id| *id == LayerId::Ime)
            .unwrap();
        assert!(ime < tty, "IME must precede the TTY stage");
        for id in [
            LayerId::Remapper,
            LayerId::Compositor,
            LayerId::Terminal,
            LayerId::Multiplexer,
            LayerId::Session,
            LayerId::Application,
            LayerId::Shell,
        ] {
            assert!(
                ROUTE_ORDER.contains(&id),
                "route order must name {id:?} explicitly"
            );
        }
    }

    #[test]
    fn emitted_layers_never_fall_back_to_an_unknown_id() {
        for key in ["ctrl+f", "ctrl+z", "super+c", "super+return", "ctrl+left"] {
            let key: KeyCombo = key.parse().unwrap();
            let layers = inspect_default_chain(&key);
            assert!(!layers.is_empty());
            for layer in &layers {
                assert_ne!(
                    layer.id,
                    LayerId::Unknown,
                    "layer {:?} must carry an explicit identity",
                    layer.layer
                );
            }
        }
    }

    #[test]
    fn ime_result_is_placed_before_the_tty_stage() {
        let tty = LayerResult::new("TTY driver", LayerId::Tty, Outcome::Pass, "tty", Vec::new());
        let ime = LayerResult::new(
            "IME",
            LayerId::Ime,
            Outcome::HandledAndPassed,
            "ime",
            Vec::new(),
        );
        let mut results = vec![tty];
        let insert_at = results
            .iter()
            .position(|result| result.id == LayerId::Tty)
            .unwrap_or(results.len());
        results.insert(insert_at, ime);
        let ids: Vec<LayerId> = results.iter().map(|result| result.id).collect();
        assert_eq!(ids, vec![LayerId::Ime, LayerId::Tty]);
    }

    #[test]
    fn unified_terminal_identity_matches_schema_context() {
        let identity = terminal_identity();
        let context = crate::schema::context();
        assert_eq!(
            context.get("terminal").and_then(|value| value.as_str()),
            identity,
            "schema context and text reports must name the same adapter"
        );
    }

    #[test]
    fn missing_byte_prediction_does_not_drop_downstream_layers() {
        let key: KeyCombo = "alt+return".parse().unwrap();
        let layers = inspect_default_chain(&key);
        let ids: Vec<LayerId> = layers.iter().map(|l| l.id).collect();
        assert!(
            ids.contains(&LayerId::Tty),
            "TTY driver must be inspected even when terminal bytes are unpredicted"
        );
        assert!(
            ids.contains(&LayerId::Session),
            "Session/multiplexer must be inspected even when terminal bytes are unpredicted"
        );
        assert!(
            ids.contains(&LayerId::Application),
            "Application layer must be inspected even when terminal bytes are unpredicted"
        );
    }

    #[test]
    fn active_application_target_does_not_append_parent_shell() {
        let key: KeyCombo = "ctrl+w".parse().unwrap();
        let mut request = InspectRequest::new(&key);
        request.force_continue = true;
        request.application_pid = Some(1);
        request.application_source = Some("test");
        let session = crate::environment::Environment::collect();
        let results = inspect_with_request_and_session(&request, &session);
        let app_result = results.iter().find(|l| l.id == LayerId::Application);
        assert!(app_result.is_some());
        let app = app_result.unwrap();
        if app.outcome != Outcome::UnadaptedTarget {
            assert!(
                results.iter().all(|l| l.id != LayerId::Shell),
                "when an interactive application is active, parent shell must not be appended"
            );
        }
    }
}
