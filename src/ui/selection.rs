//! The stream-table selection: a set of row indices plus a Shift-click anchor.
//!
//! It's a set rather than a single index so batch edits (remove three subtitle
//! tracks, retag several audio streams) take one action instead of one per row.
//! The indices point into `Project::streams`, so every mutation that shifts that
//! vec must fix the selection up too — `on_removed` is the hook for that, kept here so the shifting logic is tested once, without egui.
//!
//! Click semantics follow the usual file-manager conventions: a plain click
//! selects one row, Ctrl toggles, Shift selects a range from the anchor, and
//! Ctrl+Shift adds that range to what's already selected.

use std::collections::BTreeSet;

/// Which rows of the stream table are selected.
#[derive(Debug, Default, Clone)]
pub(crate) struct Selection {
    /// Selected indices into `project.streams`, kept sorted so batch operations
    /// can walk them in order (or reversed, for index-shifting deletes).
    rows: BTreeSet<usize>,
    /// Where a Shift-click range starts: the row of the last plain or Ctrl click.
    /// `None` until something is clicked, in which case Shift acts like a plain click.
    anchor: Option<usize>,
}

impl Selection {
    /// Apply a row click with the held modifiers (see the module doc for the rules).
    /// `len` is the stream count, used to keep a range inside the table.
    pub(crate) fn click(&mut self, i: usize, modifiers: egui::Modifiers, len: usize) {
        if i >= len {
            return;
        }
        match (modifiers.command, modifiers.shift, self.anchor) {
            (ctrl, true, Some(anchor)) => {
                let anchor = anchor.min(len - 1);
                if !ctrl {
                    self.rows.clear();
                }
                self.rows.extend(anchor.min(i)..=anchor.max(i));
                // The anchor stays put, so successive Shift-clicks re-pivot on it.
            }
            (true, _, _) => {
                if !self.rows.remove(&i) {
                    self.rows.insert(i);
                }
                self.anchor = Some(i);
            }
            _ => self.set_single(i),
        }
    }

    /// Select exactly row `i`, making it the anchor. Used after any action that
    /// produces one obvious row to focus (load, embed, reorder, add-converted).
    pub(crate) fn set_single(&mut self, i: usize) {
        self.rows.clear();
        self.rows.insert(i);
        self.anchor = Some(i);
    }

    /// Select exactly `rows`, anchoring on the first. Used after a batch insert
    /// so the new streams are what the inspector edits next.
    pub(crate) fn set_many(&mut self, rows: impl IntoIterator<Item = usize>) {
        self.rows = rows.into_iter().collect();
        self.anchor = self.rows.first().copied();
    }

    /// Deselect everything.
    pub(crate) fn clear(&mut self) {
        self.rows.clear();
        self.anchor = None;
    }

    /// Select every row of a `len`-stream table.
    pub(crate) fn select_all(&mut self, len: usize) {
        self.rows = (0..len).collect();
        self.anchor = (len > 0).then_some(0);
    }

    /// Whether row `i` is selected.
    pub(crate) fn contains(&self, i: usize) -> bool {
        self.rows.contains(&i)
    }

    /// The selected rows, ascending.
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
        self.rows.iter().copied()
    }

    /// The selected rows, ascending, as an owned vec — handy when the caller is
    /// about to mutably borrow the project the indices point into.
    pub(crate) fn to_vec(&self) -> Vec<usize> {
        self.iter().collect()
    }

    /// The selected row when exactly one is selected. Single-stream-only actions
    /// (extract, sync offset) key off this.
    pub(crate) fn single(&self) -> Option<usize> {
        match self.rows.len() {
            1 => self.rows.first().copied(),
            _ => None,
        }
    }

    /// Fix up after `streams[i]` was removed, leaving `new_len` streams. Indices
    /// above `i` shift down by one. If that emptied a selection that contained
    /// `i`, fall back to the row now at `i` (or the new last row) so the inspector
    /// keeps showing something nearby instead of going blank mid-edit.
    pub(crate) fn on_removed(&mut self, i: usize, new_len: usize) {
        let was_selected = self.rows.remove(&i);
        self.rows = self
            .rows
            .iter()
            .map(|&s| if s > i { s - 1 } else { s })
            .collect();
        self.anchor = match self.anchor {
            Some(a) if a > i => Some(a - 1),
            Some(a) if a == i => None,
            other => other,
        };
        if was_selected && self.rows.is_empty() && new_len > 0 {
            self.set_single(i.min(new_len - 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    fn rows(s: &Selection) -> Vec<usize> {
        s.to_vec()
    }

    #[test]
    fn plain_click_selects_only_that_row() {
        let mut s = Selection::default();
        s.select_all(5);
        s.click(2, Modifiers::NONE, 5);
        assert_eq!(rows(&s), [2]);
        assert_eq!(s.single(), Some(2));
    }

    #[test]
    fn ctrl_click_toggles() {
        let mut s = Selection::default();
        s.click(1, Modifiers::NONE, 5);
        s.click(3, Modifiers::COMMAND, 5);
        assert_eq!(rows(&s), [1, 3]);
        assert_eq!(s.single(), None);
        s.click(1, Modifiers::COMMAND, 5);
        assert_eq!(rows(&s), [3]);
    }

    #[test]
    fn shift_click_selects_range_from_anchor_either_direction() {
        let mut s = Selection::default();
        s.click(3, Modifiers::NONE, 6);
        s.click(5, Modifiers::SHIFT, 6);
        assert_eq!(rows(&s), [3, 4, 5]);
        // Re-pivots on the same anchor, replacing the previous range.
        s.click(1, Modifiers::SHIFT, 6);
        assert_eq!(rows(&s), [1, 2, 3]);
    }

    #[test]
    fn shift_without_anchor_acts_like_plain_click() {
        let mut s = Selection::default();
        s.click(2, Modifiers::SHIFT, 4);
        assert_eq!(rows(&s), [2]);
    }

    #[test]
    fn ctrl_shift_click_adds_range() {
        let mut s = Selection::default();
        s.click(0, Modifiers::NONE, 8);
        s.click(5, Modifiers::COMMAND, 8); // anchor now 5
        s.click(7, Modifiers::COMMAND | Modifiers::SHIFT, 8);
        assert_eq!(rows(&s), [0, 5, 6, 7]);
    }

    #[test]
    fn click_out_of_range_is_ignored() {
        let mut s = Selection::default();
        s.set_single(1);
        s.click(9, Modifiers::NONE, 3);
        assert_eq!(rows(&s), [1]);
    }

    #[test]
    fn on_removed_shifts_higher_indices_down() {
        let mut s = Selection::default();
        s.set_many([0, 2, 4]);
        s.on_removed(2, 4);
        assert_eq!(rows(&s), [0, 3]);
        s.on_removed(1, 3); // an unselected row below 3
        assert_eq!(rows(&s), [0, 2]);
    }

    #[test]
    fn on_removed_falls_back_to_a_nearby_row() {
        let mut s = Selection::default();
        s.set_single(2);
        s.on_removed(2, 2); // removed the last row of three
        assert_eq!(rows(&s), [1]);

        let mut s = Selection::default();
        s.set_single(0);
        s.on_removed(0, 0); // removed the only row
        assert!(rows(&s).is_empty());

        // Nothing was selected: removing a row must not invent a selection.
        let mut s = Selection::default();
        s.on_removed(0, 3);
        assert!(rows(&s).is_empty());
    }
}
