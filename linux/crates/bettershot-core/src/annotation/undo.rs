//! Snapshot undo and redo for the editor.

/// Keeps up to `limit` earlier states. Push the state *before* each edit.
#[derive(Debug, Clone)]
pub struct UndoStack<T> {
    undo: Vec<T>,
    redo: Vec<T>,
    limit: usize,
}

impl<T: Clone + PartialEq> UndoStack<T> {
    pub fn new(limit: usize) -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), limit }
    }

    /// Records `before` unless it equals the last recorded state.
    pub fn record(&mut self, before: T) {
        if self.undo.last() == Some(&before) {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: T) -> Option<T> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    pub fn redo(&mut self, current: T) -> Option<T> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }

    /// Drops the last recorded state without making it redoable, for an edit
    /// that turned out to change nothing (a click with a drawing tool).
    pub fn discard_last(&mut self) -> Option<T> {
        self.undo.pop()
    }

    /// The most recent recorded state.
    pub fn peek(&self) -> Option<&T> {
        self.undo.last()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_and_limits() {
        let mut stack = UndoStack::new(2);
        stack.record(1);
        stack.record(1);
        stack.record(2);
        stack.record(3);
        assert_eq!(stack.undo(4), Some(3));
        assert_eq!(stack.undo(3), Some(2));
        assert_eq!(stack.undo(2), None, "the oldest state fell off the limit");
        assert_eq!(stack.redo(2), Some(3));
        stack.record(9);
        assert!(!stack.can_redo(), "a new edit clears redo");
    }
}
