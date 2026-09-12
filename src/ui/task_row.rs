//! One row of the task list. Everything on it is reachable with a single click:
//! check to complete, click the title to edit, the dot to pick a priority, the
//! trash icon to delete (Undo lives in a toast, so no confirmation dialog).

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::settings::Settings;
use crate::task::{Priority, Task};
use crate::ui::colors;
use crate::ui::window::Ctx;

pub fn build_row(
    task: &Task,
    priorities: &[Priority],
    settings: &Settings,
    ctx: &Rc<Ctx>,
) -> gtk::ListBoxRow {
    let id = task.id;
    let fallback = Priority {
        id: -1,
        name: "Unknown".into(),
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
            "Mark as not done"
        } else {
            "Mark as done"
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
        .tooltip_text(format!("Priority: {}", current.name))
        .css_classes(["circular"])
        .build();

    // --- delete ------------------------------------------------------------
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text("Delete")
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
