//! The messages strip under the editor (docs/spec.md, "Runs and results"): every run's
//! printed text, row counts and errors in order, plus the app's own notices.
//!
//! An error that names a line can take the user there: Up and Down step between such
//! errors while the strip has focus, and Enter (or a click) jumps. The jump itself is the
//! workbench's job, since it owns the tabs; the strip only says which line was chosen.

use gpui::{EntityId, SharedString};
use serde::Serialize;

/// How a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// Printed text, notices, row counts and the app's own messages.
    Plain,
    /// Server warnings.
    Warning,
    /// Errors, drawn in red.
    Error,
}

/// The editor line an error points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Jump {
    /// The query tab whose editor holds the line.
    pub tab: EntityId,
    /// Zero-based line in that editor.
    pub line: u32,
}

/// One line of the strip.
#[derive(Clone, Debug)]
pub struct Message {
    pub text: SharedString,
    pub tone: Tone,
    pub jump: Option<Jump>,
}

impl Message {
    /// A plain line with nowhere to jump.
    pub fn plain(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Plain,
            jump: None,
        }
    }
}

/// The strip's lines and which error the keyboard has chosen.
#[derive(Default)]
pub struct Messages {
    lines: Vec<Message>,
    /// Index into `lines` of the chosen error, if any.
    chosen: Option<usize>,
}

impl Messages {
    pub fn push(&mut self, message: Message) {
        self.lines.push(message);
    }

    pub fn lines(&self) -> &[Message] {
        &self.lines
    }

    pub fn chosen(&self) -> Option<usize> {
        self.chosen
    }

    /// Chooses the next error with a line after the chosen one (`forward`) or before it.
    /// With none chosen, Up starts from the newest error and Down from the oldest. Stays put
    /// at either end.
    pub fn step(&mut self, forward: bool) {
        let jumps = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, m)| m.jump.is_some())
            .map(|(ix, _)| ix);
        let next = match (self.chosen, forward) {
            (None, true) => jumps.clone().next(),
            (None, false) => jumps.clone().next_back(),
            (Some(at), true) => jumps.clone().find(|ix| *ix > at).or(Some(at)),
            (Some(at), false) => jumps.clone().rfind(|ix| *ix < at).or(Some(at)),
        };
        self.chosen = next;
    }

    /// Chooses line `ix` and returns where it points, for a click.
    pub fn choose(&mut self, ix: usize) -> Option<Jump> {
        let jump = self.lines.get(ix)?.jump?;
        self.chosen = Some(ix);
        Some(jump)
    }

    /// Where the chosen error points, for Enter.
    pub fn chosen_jump(&self) -> Option<Jump> {
        self.lines.get(self.chosen?)?.jump
    }
}
