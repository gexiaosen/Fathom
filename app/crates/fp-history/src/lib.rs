//! Command-based undo/redo for [`fp_model::Document`].
//!
//! Every mutation that should be undoable is expressed as a [`Command`].
//! `apply` and `undo` must be exact inverses; commands that allocate new
//! entity IDs must remember them so redo does not create duplicates.

use fp_model::Document;

mod transaction;
pub use transaction::Transaction;

pub trait Command: Send {
    fn label(&self) -> &str;
    fn apply(&mut self, doc: &mut Document);
    fn undo(&mut self, doc: &mut Document);
}

pub struct History {
    undo_stack: Vec<Box<dyn Command>>,
    redo_stack: Vec<Box<dyn Command>>,
    capacity: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn execute(&mut self, mut command: Box<dyn Command>, doc: &mut Document) {
        command.apply(doc);
        self.undo_stack.push(command);
        self.redo_stack.clear();
        if self.undo_stack.len() > self.capacity {
            self.undo_stack.remove(0);
        }
    }

    pub fn undo(&mut self, doc: &mut Document) -> bool {
        let Some(mut command) = self.undo_stack.pop() else {
            return false;
        };
        command.undo(doc);
        self.redo_stack.push(command);
        true
    }

    pub fn redo(&mut self, doc: &mut Document) -> bool {
        let Some(mut command) = self.redo_stack.pop() else {
            return false;
        };
        command.apply(doc);
        self.undo_stack.push(command);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new(150)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rename {
        before: String,
        after: String,
    }

    impl Command for Rename {
        fn label(&self) -> &str {
            "rename"
        }

        fn apply(&mut self, doc: &mut Document) {
            doc.meta.name = self.after.clone();
        }

        fn undo(&mut self, doc: &mut Document) {
            doc.meta.name = self.before.clone();
        }
    }

    fn rename(from: &str, to: &str) -> Box<dyn Command> {
        Box::new(Rename {
            before: from.into(),
            after: to.into(),
        })
    }

    #[test]
    fn execute_undo_redo_roundtrip() {
        let mut doc = Document::new("A");
        let mut history = History::new(10);

        history.execute(rename("A", "B"), &mut doc);
        assert_eq!(doc.meta.name, "B");
        assert!(history.can_undo());
        assert!(!history.can_redo());

        assert!(history.undo(&mut doc));
        assert_eq!(doc.meta.name, "A");
        assert!(history.can_redo());

        assert!(history.redo(&mut doc));
        assert_eq!(doc.meta.name, "B");
    }

    #[test]
    fn new_command_clears_redo() {
        let mut doc = Document::new("A");
        let mut history = History::new(10);
        history.execute(rename("A", "B"), &mut doc);
        history.execute(rename("B", "C"), &mut doc);
        history.undo(&mut doc);
        assert!(history.can_redo());
        history.execute(rename("B", "D"), &mut doc);
        assert!(!history.can_redo());
    }

    #[test]
    fn capacity_drops_oldest() {
        let mut doc = Document::new("A");
        let mut history = History::new(2);
        history.execute(rename("A", "B"), &mut doc);
        history.execute(rename("B", "C"), &mut doc);
        history.execute(rename("C", "D"), &mut doc);
        assert_eq!(history.undo_len(), 2);
    }
}
