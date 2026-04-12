//! Tracks currently-emitted KGP placements so redraw can skip unchanged ones.
//!
//! A "placement" in Kitty Graphics Protocol is identified by (image_id, placement_id).
//! Re-sending `a=p` with an existing (i, p) pair is an atomic in-place move — no
//! flicker. This struct remembers the last-emitted parameters per logical slot so
//! `redraw()` can decide: skip, update, or delete.

use std::collections::HashMap;

/// Logical slot for a placement. Stable across frames so we can diff.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(super) enum PlacementSlot {
    Content(usize),
    Sidebar(usize),
    OverlayPrimary(usize, usize),
    OverlayOverflow(usize, usize),
}

/// Full set of parameters emitted for a single `a=p` placement command.
///
/// Two specs that compare equal produce byte-identical KGP output, so the ledger
/// can skip re-emission. Fields map 1:1 onto KGP params.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(super) struct PlacementSpec {
    pub image_id: u32,
    pub placement_id: u32,
    pub screen_col: u16,
    pub screen_row: u16,
    pub cols: u16,
    pub rows: u16,
    /// Source-rect X (pixels, into image).
    pub src_x: u32,
    /// Source-rect Y (pixels, into image).
    pub src_y: u32,
    /// Source-rect width (pixels).
    pub src_w: u32,
    /// Source-rect height (pixels).
    pub src_h: u32,
    /// Sub-cell X offset (pixels).
    pub x_off: u32,
    /// Sub-cell Y offset (pixels).
    pub y_off: u32,
    /// Z-index (0 for tiles, 1 for overlays).
    pub z: i32,
}

/// Outcome of diffing a desired spec against the ledger.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum DiffOp {
    /// Placement already matches — do nothing.
    Skip,
    /// Placement is new or changed — emit `a=p` (Kitty replaces in-place if same (i,p)).
    Upsert,
}

/// Tracks last-emitted placement parameters by slot.
#[derive(Default)]
pub(super) struct PlacementLedger {
    entries: HashMap<PlacementSlot, PlacementSpec>,
}

impl PlacementLedger {
    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Diff `desired` against the stored entry for `slot` without mutating.
    pub(super) fn diff(&self, slot: PlacementSlot, desired: &PlacementSpec) -> DiffOp {
        match self.entries.get(&slot) {
            Some(prev) if prev == desired => DiffOp::Skip,
            _ => DiffOp::Upsert,
        }
    }

    /// Record that `spec` is now the live placement for `slot`. Call after
    /// successfully writing to stdout.
    pub(super) fn record(&mut self, slot: PlacementSlot, spec: PlacementSpec) {
        self.entries.insert(slot, spec);
    }

    /// Remove and return entries whose slots are not in `keep`. Returned entries
    /// identify placements that must be deleted from the terminal.
    pub(super) fn retain_slots(
        &mut self,
        keep: &std::collections::HashSet<PlacementSlot>,
    ) -> Vec<(PlacementSlot, PlacementSpec)> {
        let to_remove: Vec<PlacementSlot> = self
            .entries
            .keys()
            .filter(|s| !keep.contains(*s))
            .copied()
            .collect();
        to_remove
            .into_iter()
            .filter_map(|s| self.entries.remove(&s).map(|v| (s, v)))
            .collect()
    }

    /// Drop every tracked placement without emitting anything (use after
    /// `delete_all_images` or when the image IDs are about to be invalidated).
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(image_id: u32, screen_row: u16) -> PlacementSpec {
        PlacementSpec {
            image_id,
            placement_id: 1,
            screen_col: 0,
            screen_row,
            cols: 80,
            rows: 20,
            src_x: 0,
            src_y: 0,
            src_w: 800,
            src_h: 500,
            x_off: 0,
            y_off: 0,
            z: 0,
        }
    }

    #[test]
    fn new_ledger_is_empty() {
        let l = PlacementLedger::new();
        assert_eq!(l.len(), 0);
    }

    #[test]
    fn diff_unseen_slot_is_upsert() {
        let l = PlacementLedger::new();
        assert_eq!(
            l.diff(PlacementSlot::Content(0), &spec(1, 0)),
            DiffOp::Upsert
        );
    }

    #[test]
    fn diff_same_spec_is_skip() {
        let mut l = PlacementLedger::new();
        let s = spec(1, 0);
        l.record(PlacementSlot::Content(0), s);
        assert_eq!(l.diff(PlacementSlot::Content(0), &s), DiffOp::Skip);
    }

    #[test]
    fn diff_changed_spec_is_upsert() {
        let mut l = PlacementLedger::new();
        l.record(PlacementSlot::Content(0), spec(1, 0));
        assert_eq!(
            l.diff(PlacementSlot::Content(0), &spec(1, 5)),
            DiffOp::Upsert
        );
    }

    #[test]
    fn retain_slots_returns_removed() {
        use std::collections::HashSet;
        let mut l = PlacementLedger::new();
        l.record(PlacementSlot::Content(0), spec(1, 0));
        l.record(PlacementSlot::Content(1), spec(2, 10));
        l.record(PlacementSlot::Sidebar(0), spec(3, 0));

        let mut keep = HashSet::new();
        keep.insert(PlacementSlot::Content(0));
        keep.insert(PlacementSlot::Sidebar(0));

        let removed = l.retain_slots(&keep);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0, PlacementSlot::Content(1));
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn clear_drops_everything() {
        let mut l = PlacementLedger::new();
        l.record(PlacementSlot::Content(0), spec(1, 0));
        l.clear();
        assert_eq!(l.len(), 0);
    }
}
