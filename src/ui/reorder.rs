//! Drag-and-drop reordering for `gtk::ListBox` rows, independent of what the
//! rows contain. Used by the priorities list in Preferences; the task list can
//! use it the same way.
//!
//! Usage: create one `Reorder` per list with a callback, then call `attach`
//! for every row you build, passing the widget that acts as the drag handle.
//! On drop the callback receives `(from, to)` row indices; the owner reorders
//! its data and re-renders — no widgets are moved here, which keeps the
//! "rebuild the list on every change" model intact.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};

const CLASS_ABOVE: &str = "drop-above";
const CLASS_BELOW: &str = "drop-below";

#[derive(Clone)]
pub struct Reorder {
    on_move: Rc<dyn Fn(usize, usize)>,
}

impl Reorder {
    pub fn new(on_move: impl Fn(usize, usize) + 'static) -> Self {
        Self {
            on_move: Rc::new(on_move),
        }
    }

    /// The conventional grip icon, ready to be used as a handle.
    pub fn handle() -> gtk::Image {
        let image = gtk::Image::from_icon_name("list-drag-handle-symbolic");
        image.add_css_class("drag-handle");
        image.set_tooltip_text(Some("Drag to reorder"));
        image.set_valign(gtk::Align::Center);
        image.set_can_target(true);
        image.set_cursor_from_name(Some("grab"));
        image
    }

    /// Makes `row` draggable by `handle` and a drop target for its siblings.
    pub fn attach(&self, row: &gtk::ListBoxRow, handle: &impl IsA<gtk::Widget>) {
        // --- drag source: the payload is the row object itself ---------------
        let source = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        source.connect_prepare(glib::clone!(
            #[weak]
            row,
            #[upgrade_or]
            None,
            move |_, _, _| Some(gdk::ContentProvider::for_value(&row.to_value()))
        ));
        source.connect_drag_begin(glib::clone!(
            #[weak]
            row,
            move |source, _| {
                let paintable = gtk::WidgetPaintable::new(Some(&row));
                source.set_icon(Some(&paintable), 0, 0);
                row.add_css_class("dragging");
            }
        ));
        source.connect_drag_end(glib::clone!(
            #[weak]
            row,
            move |_, _, _| row.remove_css_class("dragging")
        ));
        handle.add_controller(source);

        // --- drop target: accept rows from the same list -----------------------
        let target = gtk::DropTarget::new(gtk::ListBoxRow::static_type(), gdk::DragAction::MOVE);
        target.set_preload(true);
        target.connect_motion(glib::clone!(
            #[weak]
            row,
            #[upgrade_or]
            gdk::DragAction::empty(),
            move |_, _, y| {
                show_indicator(&row, is_upper_half(&row, y));
                gdk::DragAction::MOVE
            }
        ));
        target.connect_leave(glib::clone!(
            #[weak]
            row,
            move |_| clear_indicator(&row)
        ));
        let on_move = Rc::clone(&self.on_move);
        target.connect_drop(glib::clone!(
            #[weak]
            row,
            #[upgrade_or]
            false,
            move |_, value, _, y| {
                clear_indicator(&row);
                let Ok(dragged) = value.get::<gtk::ListBoxRow>() else {
                    return false;
                };
                if dragged.parent() != row.parent() {
                    return false; // from another list
                }
                let (Ok(from), Ok(over)) = (
                    usize::try_from(dragged.index()),
                    usize::try_from(row.index()),
                ) else {
                    return false;
                };
                let to = final_index(from, over, is_upper_half(&row, y));
                if to != from {
                    on_move(from, to);
                }
                true
            }
        ));
        row.add_controller(target);
    }
}

/// Where a row dragged from index `from` ends up when dropped on the row at
/// index `over`, either in its upper half (`above`) or lower half.
pub fn final_index(from: usize, over: usize, above: bool) -> usize {
    let insert_at = if above { over } else { over + 1 };
    if from < insert_at {
        insert_at - 1
    } else {
        insert_at
    }
}

fn is_upper_half(row: &gtk::ListBoxRow, y: f64) -> bool {
    y < f64::from(row.height()) / 2.0
}

fn show_indicator(row: &gtk::ListBoxRow, above: bool) {
    let (add, remove) = if above {
        (CLASS_ABOVE, CLASS_BELOW)
    } else {
        (CLASS_BELOW, CLASS_ABOVE)
    };
    row.remove_css_class(remove);
    row.add_css_class(add);
}

fn clear_indicator(row: &gtk::ListBoxRow) {
    row.remove_css_class(CLASS_ABOVE);
    row.remove_css_class(CLASS_BELOW);
}

#[cfg(test)]
mod tests {
    use super::final_index;

    #[test]
    fn drop_positions_map_to_final_indices() {
        // Dropping on itself (either half) is a no-op.
        assert_eq!(final_index(2, 2, true), 2);
        assert_eq!(final_index(2, 2, false), 2);
        // Moving down: above row 3 lands at 2, below row 3 lands at 3.
        assert_eq!(final_index(0, 3, true), 2);
        assert_eq!(final_index(0, 3, false), 3);
        // Moving up: above row 0 lands at 0, below row 0 lands at 1.
        assert_eq!(final_index(3, 0, true), 0);
        assert_eq!(final_index(3, 0, false), 1);
        // Adjacent rows: below the next row is one step down.
        assert_eq!(final_index(1, 2, false), 2);
        assert_eq!(final_index(1, 2, true), 1);
    }
}
