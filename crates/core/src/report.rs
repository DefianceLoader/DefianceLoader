//! Where the core's progress lines go.
//!
//! `apply` used to `println!` them, which is fine in the injector's console but
//! not in the loader, where the same code runs inside a GUI process with no
//! stdout. A sink is set once: the injector points it at `println!`, the loader
//! at `defiance-loader.log`. Unset means the lines are dropped, which is safe.

use std::sync::OnceLock;

type Sink = Box<dyn Fn(&str) + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();

/// Point the core's notes at `sink`. Only the first call has an effect.
pub fn set(sink: Sink) {
    let _ = SINK.set(sink);
}

/// One progress line.
pub fn note(message: String) {
    if let Some(sink) = SINK.get() {
        sink(&message);
    }
}
