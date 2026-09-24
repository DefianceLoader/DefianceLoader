//! Where the core's progress lines go.
//!
//! The same code runs in the injector's console and inside the game, a GUI
//! process with no stdout, so it never prints. A sink is set once: the injector
//! points it at `println!`, the loader at `defiance-loader.log`. Unset means the
//! lines are dropped, which is safe.

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
