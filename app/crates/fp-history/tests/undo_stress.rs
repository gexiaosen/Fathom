//! Undo/redo stress tests: 100 edits must unwind cleanly and never grow the
//! document beyond what was there before.

use fp_history::{Command, History, Transaction};
use fp_model::Document;

struct SetName {
    before: String,
    after: String,
}

impl Command for SetName {
    fn label(&self) -> &str {
        "set-name"
    }

    fn apply(&mut self, doc: &mut Document) {
        doc.meta.name = self.after.clone();
    }

    fn undo(&mut self, doc: &mut Document) {
        doc.meta.name = self.before.clone();
    }
}

fn set_name(from: &str, to: &str) -> Box<dyn Command> {
    Box::new(SetName {
        before: from.into(),
        after: to.into(),
    })
}

#[test]
fn hundred_undos_restore_the_initial_state() {
    let mut doc = Document::new("initial");
    let mut history = History::new(200);

    let mut current = "initial".to_owned();
    for i in 0..100 {
        let next = format!("edit-{i}");
        history.execute(set_name(&current, &next), &mut doc);
        current = next;
    }
    assert_eq!(doc.meta.name, "edit-99");
    assert_eq!(history.undo_len(), 100);

    for _ in 0..100 {
        assert!(history.undo(&mut doc), "undo should not run dry");
    }
    assert_eq!(doc.meta.name, "initial");
    assert_eq!(history.undo_len(), 0);
    assert_eq!(history.redo_len(), 100);
    assert!(!history.can_undo());
}

#[test]
fn repeated_cycles_do_not_accumulate_state() {
    let mut doc = Document::new("initial");
    let mut history = History::new(150);

    for cycle in 0..10 {
        for i in 0..100 {
            let from = if i == 0 {
                "initial".to_owned()
            } else {
                format!("c{cycle}-{i}")
            };
            history.execute(set_name(&from, &format!("c{cycle}-{}", i + 1)), &mut doc);
        }
        for _ in 0..100 {
            assert!(history.undo(&mut doc));
        }
        assert_eq!(doc.meta.name, "initial");
        assert_eq!(history.undo_len(), 0);
    }

    assert_eq!(doc.walls.len(), 0, "no stray geometry");
    assert_eq!(doc.meta.name, "initial");
}

#[test]
fn transaction_collapses_a_batch_into_one_step() {
    let mut doc = Document::new("initial");
    let mut history = History::new(50);

    let mut tx = Transaction::new("拖拽");
    for i in 0..30 {
        let from = if i == 0 {
            "initial".to_owned()
        } else {
            format!("drag-{i}")
        };
        let to = format!("drag-{}", i + 1);
        tx.push(set_name(&from, &to));
    }
    history.execute(Box::new(tx), &mut doc);

    assert_eq!(history.undo_len(), 1);
    assert!(history.undo(&mut doc));
    assert_eq!(doc.meta.name, "initial");
}

#[test]
fn capacity_is_enforced_under_pressure() {
    let mut doc = Document::new("initial");
    let mut history = History::new(20);

    for i in 0..100 {
        history.execute(set_name("initial", &format!("v{i}")), &mut doc);
    }
    assert_eq!(history.undo_len(), 20, "oldest commands are dropped");

    for _ in 0..20 {
        assert!(history.undo(&mut doc));
    }
    assert!(!history.can_undo());
}
