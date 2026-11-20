//! Undo and redo as document snapshots. A run of typing or of erasing is one step, so the
//! draft is copied once per run, never once per keystroke.

use std::ops::Range;

use super::doc::Doc;

/// Steps kept; the oldest is dropped past this.
const DEPTH: usize = 100;

/// What one undo step restores: the document and where the selection was.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Snapshot {
    pub doc: Doc,
    pub selection: Range<usize>,
}

/// What the next change is, so consecutive ones of a kind share a step.
pub(super) enum Change {
    /// One character typed at `at`; `word_start` begins a new step.
    Type {
        at: usize,
        len: usize,
        word_start: bool,
    },
    /// One character erased.
    Erase {
        range: Range<usize>,
    },
    Other,
}

#[derive(Default)]
enum Run {
    #[default]
    Idle,
    Typing {
        end: usize,
    },
    Erasing {
        at: usize,
    },
}

#[derive(Default)]
pub(super) struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    run: Run,
}

impl History {
    /// Call before applying `change` to `now`: saves it unless the change continues the run.
    pub fn record(&mut self, now: &Snapshot, change: Change) {
        let continues = match (&self.run, &change) {
            (
                Run::Typing { end },
                Change::Type {
                    at,
                    word_start: false,
                    ..
                },
            ) => end == at,
            // Backspace erases just before the last erasure, forward delete at it.
            (Run::Erasing { at }, Change::Erase { range }) => {
                range.end == *at || range.start == *at
            }
            _ => false,
        };
        if !continues {
            self.push(now);
        }
        self.run = match change {
            Change::Type { at, len, .. } => Run::Typing { end: at + len },
            Change::Erase { range } => Run::Erasing { at: range.start },
            Change::Other => Run::Idle,
        };
    }

    /// Save `now` as its own step.
    pub fn checkpoint(&mut self, now: &Snapshot) {
        self.push(now);
        self.run = Run::Idle;
    }

    /// The next change starts a new step (the caret moved).
    pub fn break_run(&mut self) {
        self.run = Run::Idle;
    }

    fn push(&mut self, now: &Snapshot) {
        if self.undo.len() == DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(now.clone());
        self.redo.clear();
    }

    /// Swap `now` for the previous step; false when there is none.
    pub fn undo(&mut self, now: &mut Snapshot) -> bool {
        self.run = Run::Idle;
        exchange(&mut self.undo, &mut self.redo, now)
    }

    /// Swap `now` for the step last undone; false when there is none.
    pub fn redo(&mut self, now: &mut Snapshot) -> bool {
        self.run = Run::Idle;
        exchange(&mut self.redo, &mut self.undo, now)
    }
}

fn exchange(from: &mut Vec<Snapshot>, to: &mut Vec<Snapshot>, now: &mut Snapshot) -> bool {
    let Some(target) = from.pop() else {
        return false;
    };
    to.push(std::mem::replace(now, target));
    true
}
