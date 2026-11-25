//! The host-side model the canvas edits: a `mold_graph::Editor` plus the evaluation
//! badges. The canvas holds a handle to it, never a copy of the graph.

use std::sync::Arc;

use mold_graph::{Edit, Editor, Graph, GraphError, Registry};

use super::eval::{EvalStates, changed_nodes};

/// A graph under edit (with undo/redo) and the per-node evaluation states shown on
/// the canvas. Every change goes through [`Editor`], so every change is undoable.
///
/// After an edit, undo or redo, the nodes that changed and everything downstream of
/// them are marked [`Dirty`](super::EvalState::Dirty): "changed since the last
/// evaluation the canvas was told about". The host's next `Engine::generate` (fed
/// in with [`EvalStates::from_item_graph`]) gives the exact picture, e.g. cache hits
/// after an undo.
#[derive(Debug, Clone)]
pub struct GraphDoc {
    editor: Editor,
    eval: EvalStates,
}

impl GraphDoc {
    pub fn new(editor: Editor) -> Self {
        Self {
            editor,
            eval: EvalStates::new(),
        }
    }

    pub fn empty(registry: Arc<Registry>) -> Self {
        Self::new(Editor::new(registry))
    }

    pub fn graph(&self) -> &Graph {
        self.editor.graph()
    }

    pub fn registry(&self) -> &Arc<Registry> {
        self.editor.registry()
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    pub fn eval(&self) -> &EvalStates {
        &self.eval
    }

    /// For the host: replace badges from an engine run, or set them by hand.
    pub fn eval_mut(&mut self) -> &mut EvalStates {
        &mut self.eval
    }

    /// Apply one undoable edit. On error the graph is unchanged.
    pub fn apply(&mut self, edit: Edit) -> Result<(), GraphError> {
        self.tracked(|e| e.apply(edit).map(|()| true)).map(|_| ())
    }

    /// `Ok(false)` if there was nothing to undo.
    pub fn undo(&mut self) -> Result<bool, GraphError> {
        self.tracked(Editor::undo)
    }

    /// `Ok(false)` if there was nothing to redo.
    pub fn redo(&mut self) -> Result<bool, GraphError> {
        self.tracked(Editor::redo)
    }

    fn tracked(
        &mut self,
        f: impl FnOnce(&mut Editor) -> Result<bool, GraphError>,
    ) -> Result<bool, GraphError> {
        let before = self.editor.graph().clone();
        let changed = f(&mut self.editor)?;
        if changed {
            let after = self.editor.graph();
            let nodes = changed_nodes(&before, after);
            self.eval
                .mark_dirty(after, nodes.iter().filter(|n| after.contains(n)));
            self.eval.retain_in(after);
        }
        Ok(changed)
    }
}
