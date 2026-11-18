//! A query tab's messages strip (docs/spec.md, "Runs and results"): its runs' printed text,
//! row counts and errors in order, plus the app's notices while the tab is shown.
//!
//! An error that names a line can take the user there: Up and Down step between such
//! errors while the strip has focus, and Enter (or a click) jumps. The strip only says which
//! line was chosen; moving the caret is the tab's job.

use gpui::SharedString;
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

/// One line of the strip.
#[derive(Clone, Debug)]
pub struct Message {
    pub text: SharedString,
    pub tone: Tone,
    /// The zero-based editor line the message points at, if any.
    pub line: Option<u32>,
}

impl Message {
    /// A plain line with nowhere to jump.
    pub fn plain(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Plain,
            line: None,
        }
    }

    /// True for an error that points at a line: the lines Up and Down step between.
    fn is_error_with_line(&self) -> bool {
        self.tone == Tone::Error && self.line.is_some()
    }
}

/// The strip's lines and which error the keyboard has chosen.
#[derive(Default)]
pub struct Messages {
    lines: Vec<Message>,
    /// Index into `lines` of the chosen message, if any.
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

    /// Chooses the next error with a line after the chosen message (`forward`) or before it.
    /// With none chosen, Up starts from the newest error and Down from the oldest. Stays put
    /// at either end. Notices and warnings with lines are skipped.
    pub fn step(&mut self, forward: bool) {
        let mut errors = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, m)| m.is_error_with_line())
            .map(|(ix, _)| ix);
        self.chosen = match (self.chosen, forward) {
            (None, true) => errors.next(),
            (None, false) => errors.next_back(),
            (Some(at), true) => errors.find(|ix| *ix > at).or(Some(at)),
            (Some(at), false) => errors.rfind(|ix| *ix < at).or(Some(at)),
        };
    }

    /// Chooses message `ix` and returns the editor line it points at, for a click.
    pub fn choose(&mut self, ix: usize) -> Option<u32> {
        let line = self.lines.get(ix)?.line?;
        self.chosen = Some(ix);
        Some(line)
    }

    /// The editor line the chosen message points at, for Enter.
    pub fn chosen_line(&self) -> Option<u32> {
        self.lines.get(self.chosen?)?.line
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, Messages, Tone};

    fn at(text: &str, tone: Tone, line: u32) -> Message {
        Message {
            text: text.into(),
            tone,
            line: Some(line),
        }
    }

    #[test]
    fn up_skips_a_print_that_names_a_line_and_lands_on_the_error() {
        let mut messages = Messages::default();
        messages.push(at("Line 3: Incorrect syntax", Tone::Error, 2));
        messages.push(at("printed on line 5", Tone::Plain, 4));
        messages.step(false);
        assert_eq!(messages.chosen_line(), Some(2));
    }
}
