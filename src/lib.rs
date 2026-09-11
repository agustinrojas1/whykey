#![cfg_attr(
    not(target_os = "linux"),
    doc = "This crate is supported on Linux only; see the build error below."
)]
#[cfg(not(target_os = "linux"))]
compile_error!(
    "whykey currently supports Linux only; install a Linux build or use the Linux package"
);

pub mod bindings;
pub mod capabilities;
pub mod capture;
pub mod command;
pub mod conflicts;
pub mod diff;
pub mod environment;
pub mod extension_adapters;
pub mod extensions;
pub mod focus;
pub mod hyprland_capture;
pub mod ime;
pub mod key;
pub mod layers;
pub mod listen;
pub mod registry;
pub mod remapper;
pub mod replay;
pub mod report;
pub mod schema;
pub mod snapshot;
pub mod sway_capture;
pub mod util;
pub mod xkb;
