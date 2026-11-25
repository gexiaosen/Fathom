//! Group multiple commands into one atomic undo step.

use crate::{Command, Document};

/// A transaction applies its commands in order and undoes them in reverse.
/// Use it for drag gestures: many intermediate moves, one Ctrl+Z.
pub struct Transaction {
    label: String,
    commands: Vec<Box<dyn Command>>,
}

impl Transaction {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            commands: Vec::new(),
        }
    }

    pub fn push(&mut self, command: Box<dyn Command>) {
        self.commands.push(command);
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl Command for Transaction {
    fn label(&self) -> &str {
        &self.label
    }

    fn apply(&mut self, doc: &mut Document) {
        for command in &mut self.commands {
            command.apply(doc);
        }
    }

    fn undo(&mut self, doc: &mut Document) {
        for command in self.commands.iter_mut().rev() {
            command.undo(doc);
        }
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
    fn transaction_undoes_all_commands_at_once() {
        let mut doc = Document::new("A");
        let mut history = crate::History::new(10);

        let mut tx = Transaction::new("批量改名");
        tx.push(rename("A", "B"));
        tx.push(rename("B", "C"));
        tx.push(rename("C", "D"));
        assert_eq!(tx.len(), 3);

        history.execute(Box::new(tx), &mut doc);
        assert_eq!(doc.meta.name, "D");
        assert_eq!(history.undo_len(), 1);

        assert!(history.undo(&mut doc));
        assert_eq!(doc.meta.name, "A");
        assert_eq!(history.undo_len(), 0);
    }

    #[test]
    fn empty_transaction_is_a_noop() {
        let mut doc = Document::new("A");
        let mut history = crate::History::new(10);
        let tx = Transaction::new("空事务");
        assert!(tx.is_empty());
        history.execute(Box::new(tx), &mut doc);
        assert_eq!(doc.meta.name, "A");
        assert!(history.can_undo());
    }
}
