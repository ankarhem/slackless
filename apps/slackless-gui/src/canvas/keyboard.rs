//! Pure keyboard focus region & block navigation state machine for Canvases
//! (`F-COLLAB-194`, `F-COLLAB-195`, `F-COLLAB-196`).

use crate::canvas::model::{CanvasBlock, CanvasDoc, CanvasEmbedKind, MediaKind};

/// Keyboard focus regions cycled with `F6` / `Shift+F6` (`F-COLLAB-194`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CanvasFocusRegion {
    #[default]
    Header,
    Outline,
    Document,
    Comments,
}

/// Per-canvas keyboard focus & section-menu state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CanvasKeyboardState {
    pub region: CanvasFocusRegion,
    pub focused_block: Option<usize>,
    pub section_menu_block: Option<usize>,
}

/// Action triggered by pressing `Enter` or `Space` on a focused canvas block (`F-COLLAB-195`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasActivation {
    ToggleChecklist {
        block_ix: usize,
        item_ix: usize,
    },
    OpenMedia {
        block_ix: usize,
        kind: MediaKind,
        url: String,
    },
    OpenDocEmbed {
        block_ix: usize,
        kind: CanvasEmbedKind,
        file_id: String,
        url: String,
    },
    OpenProfile {
        block_ix: usize,
        user_id: String,
    },
}

/// Cycle keyboard focus forward (`F6`) or backward (`Shift+F6`) across currently visible regions
/// (`F-COLLAB-194`).
pub fn cycle_region(
    current: CanvasFocusRegion,
    forward: bool,
    has_outline: bool,
    comments_open: bool,
) -> CanvasFocusRegion {
    let mut regions = Vec::with_capacity(4);
    regions.push(CanvasFocusRegion::Header);
    if has_outline {
        regions.push(CanvasFocusRegion::Outline);
    }
    regions.push(CanvasFocusRegion::Document);
    if comments_open {
        regions.push(CanvasFocusRegion::Comments);
    }

    let pos = regions.iter().position(|r| *r == current).unwrap_or(0);
    let len = regions.len();
    let next = if forward {
        (pos + 1) % len
    } else {
        (pos + len - 1) % len
    };
    regions[next]
}

/// Move `focused_block` by `delta` (`-1` for Up, `+1` for Down) within `0..block_count`
/// (`F-COLLAB-194`).
pub fn step_block(current: Option<usize>, delta: isize, block_count: usize) -> Option<usize> {
    if block_count == 0 {
        return None;
    }
    match current {
        None => {
            if delta >= 0 {
                Some(0)
            } else {
                Some(block_count - 1)
            }
        }
        Some(ix) => {
            let clamped = (ix as isize + delta).clamp(0, (block_count - 1) as isize);
            Some(clamped as usize)
        }
    }
}

/// Compute the keyboard activation action for the focused block (`Enter` / `Space`, `F-COLLAB-195`).
pub fn activate_focused_block(
    doc: &CanvasDoc,
    focused_block: Option<usize>,
) -> Option<CanvasActivation> {
    let block_ix = focused_block?;
    let block = doc.blocks.get(block_ix)?;
    match block {
        CanvasBlock::Checklist(items) if !items.is_empty() => {
            // Toggle the first unchecked item, or item 0 if all are checked.
            let item_ix = items.iter().position(|(done, _)| !*done).unwrap_or(0);
            Some(CanvasActivation::ToggleChecklist { block_ix, item_ix })
        }
        CanvasBlock::MediaEmbed { kind, url, .. } => Some(CanvasActivation::OpenMedia {
            block_ix,
            kind: *kind,
            url: url.clone(),
        }),
        CanvasBlock::DocEmbed {
            kind, file_id, url, ..
        } => Some(CanvasActivation::OpenDocEmbed {
            block_ix,
            kind: *kind,
            file_id: file_id.clone(),
            url: url.clone(),
        }),
        CanvasBlock::ProfileCard { user_id, .. } => Some(CanvasActivation::OpenProfile {
            block_ix,
            user_id: user_id.clone(),
        }),
        _ => None,
    }
}

/// Toggle the inline Section Actions menu for the focused block (`Cmd+Shift+.` / `.`, `F-COLLAB-196`).
/// Returns the block index whose menu is now open, or `None` if closed.
pub fn toggle_section_actions_shortcut(
    state: &mut CanvasKeyboardState,
    block_count: usize,
) -> Option<usize> {
    if block_count == 0 {
        state.section_menu_block = None;
        return None;
    }
    let target = state
        .focused_block
        .map(|ix| ix.min(block_count - 1))
        .unwrap_or(0);
    state.region = CanvasFocusRegion::Document;
    state.focused_block = Some(target);
    if state.section_menu_block == Some(target) {
        state.section_menu_block = None;
    } else {
        state.section_menu_block = Some(target);
    }
    state.section_menu_block
}
