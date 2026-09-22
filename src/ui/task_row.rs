//! One row of the task list. Everything on it is reachable with a single click:
//! check to complete, click the title to edit, the dot to pick a priority, the
//! bookmark to pick labels, the trash icon to delete (Undo lives in a toast, so
//! no confirmation dialog).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::settings::Settings;
use crate::task::{Label, Priority, Task};
use crate::ui::colors;
use crate::ui::window::Ctx;

/// Label dots shown before the rest collapse into a "+N".
const MAX_DOTS: usize = 3;

pub fn build_row(
    task: &Task,
    priorities: &[Priority],
    labels: &[Label],
    settings: &Settings,
    ctx: &Rc<Ctx>,
) -> gtk::ListBoxRow {
    let id = task.id;
    let fallback = Priority {
        id: -1,
        name: crate::tr!("task.unknown_priority"),
        color: "#9a9996".into(),
        position: i64::MAX,
    };
    let current = priorities
        .iter()
        .find(|p| p.id == task.priority_id)
        .unwrap_or(&fallback);

    // --- done toggle ------------------------------------------------------
    let check = gtk::CheckButton::builder()
        .active(task.done)
        .valign(gtk::Align::Center)
        .tooltip_text(if task.done {
            crate::tr!("task.mark_not_done")
        } else {
            crate::tr!("task.mark_done")
        })
        .build();
    check.connect_toggled(glib::clone!(
        #[weak]
        ctx,
        move |check| {
            let done = check.is_active();
            ctx.later(move |ctx| ctx.set_done(id, done));
        }
    ));

    // --- title, editable in place ------------------------------------------
    let title = gtk::EditableLabel::builder()
        .text(&task.title)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .xalign(0.0)
        .build();
    let original = task.title.clone();
    title.connect_editing_notify(glib::clone!(
        #[weak]
        ctx,
        move |label| {
            if label.is_editing() {
                return; // editing just started
            }
            let text = label.text().trim().to_string();
            if text.is_empty() {
                label.set_text(&original); // refuse to blank a task
            } else if text != original {
                ctx.later(move |ctx| ctx.set_title(id, &text));
            }
        }
    ));

    // --- priority picker ---------------------------------------------------
    let popover = gtk::Popover::new();
    let picker = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    for priority in priorities {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .build();
        content.append(&colors::dot(&priority.color));
        content.append(
            &gtk::Label::builder()
                .label(&priority.name)
                .hexpand(true)
                .xalign(0.0)
                .build(),
        );
        if priority.id == current.id {
            content.append(&gtk::Image::from_icon_name("object-select-symbolic"));
        }
        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["flat"])
            .build();
        let priority_id = priority.id;
        button.connect_clicked(glib::clone!(
            #[weak]
            ctx,
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                ctx.later(move |ctx| ctx.set_priority(id, priority_id));
            }
        ));
        picker.append(&button);
    }
    popover.set_child(Some(&picker));
    let priority_button = gtk::MenuButton::builder()
        .child(&colors::dot(&current.color))
        .popover(&popover)
        .has_frame(false)
        .valign(gtk::Align::Center)
        .tooltip_text(crate::tr!("task.priority", name => current.name))
        .css_classes(["circular"])
        .build();

    // --- delete ------------------------------------------------------------
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text(crate::tr!("task.delete"))
        .css_classes(["flat", "circular"])
        .build();
    let snapshot = task.clone();
    delete.connect_clicked(glib::clone!(
        #[weak]
        ctx,
        move |_| {
            let snapshot = snapshot.clone();
            ctx.later(move |ctx| ctx.delete_task(snapshot));
        }
    ));

    // --- labels ------------------------------------------------------------
    // Walk the catalogue rather than the task's ids: the dots are then always
    // alphabetical, and an id whose label just went away drops out quietly.
    let attached: Vec<&Label> = labels
        .iter()
        .filter(|label| task.label_ids.contains(&label.id))
        .collect();
    let dots = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(3)
        .valign(gtk::Align::Center)
        .build();
    for label in attached.iter().take(MAX_DOTS) {
        let dot = colors::dot_sized(&label.color, 9);
        dot.set_tooltip_text(Some(&label.name));
        dots.append(&dot);
    }
    if attached.len() > MAX_DOTS {
        let rest: Vec<&str> = attached[MAX_DOTS..]
            .iter()
            .map(|l| l.name.as_str())
            .collect();
        dots.append(
            &gtk::Label::builder()
                .label(format!("+{}", rest.len()))
                .tooltip_text(rest.join(", "))
                .valign(gtk::Align::Center)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
    }

    // --- assemble ----------------------------------------------------------
    let hbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_start(8)
        .margin_end(4)
        .margin_top(2)
        .margin_bottom(2)
        .build();
    hbox.append(&check);
    hbox.append(&title);
    if settings.labels_enabled {
        // An empty box would still take its share of the box spacing.
        if !attached.is_empty() {
            hbox.append(&dots);
        }
        // Nothing to pick from until at least one label exists.
        if !labels.is_empty() {
            hbox.append(&label_button(id, &attached, labels, ctx));
        }
    }
    if settings.priorities_enabled {
        hbox.append(&priority_button);
    }
    hbox.append(&delete);

    let row = gtk::ListBoxRow::builder()
        .child(&hbox)
        .activatable(false)
        .selectable(false)
        .build();
    if task.done {
        row.add_css_class("done");
    }
    row
}

/// The bookmark button and its label picker. Ticking boxes changes nothing
/// until the popover closes: writing on every tick would rebuild the list, and
/// the rebuild would destroy the popover being ticked. One write, one rebuild,
/// by which time the popover is already gone.
fn label_button(id: i64, attached: &[&Label], labels: &[Label], ctx: &Rc<Ctx>) -> gtk::MenuButton {
    let before: Vec<i64> = attached.iter().map(|l| l.id).collect();
    let chosen = Rc::new(RefCell::new(before.clone()));

    let picker = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    for label in labels {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .build();
        content.append(&colors::dot(&label.color));
        content.append(
            &gtk::Label::builder()
                .label(&label.name)
                .hexpand(true)
                .xalign(0.0)
                .build(),
        );
        // `active` in the builder, before connecting, so the initial state is
        // never mistaken for a tick.
        let check = gtk::CheckButton::builder()
            .child(&content)
            .active(before.contains(&label.id))
            .build();
        let label_id = label.id;
        check.connect_toggled(glib::clone!(
            #[strong]
            chosen,
            move |check| {
                let mut chosen = chosen.borrow_mut();
                if check.is_active() {
                    chosen.push(label_id);
                } else {
                    chosen.retain(|id| *id != label_id);
                }
            }
        ));
        picker.append(&check);
    }

    let popover = gtk::Popover::new();
    popover.set_child(Some(&picker));
    popover.connect_closed(glib::clone!(
        #[weak]
        ctx,
        #[strong]
        chosen,
        move |_| {
            let mut after = chosen.borrow().clone();
            after.sort_unstable();
            let mut before = before.clone();
            before.sort_unstable();
            if after != before {
                ctx.later(move |ctx| ctx.set_task_labels(id, &after));
            }
        }
    ));

    let tooltip = if attached.is_empty() {
        crate::tr!("task.labels_none")
    } else {
        let names: Vec<&str> = attached.iter().map(|l| l.name.as_str()).collect();
        crate::tr!("task.labels", names => names.join(", "))
    };
    gtk::MenuButton::builder()
        .icon_name("user-bookmarks-symbolic")
        .popover(&popover)
        .has_frame(false)
        .valign(gtk::Align::Center)
        .tooltip_text(tooltip)
        .css_classes(["flat", "circular"])
        .build()
}
