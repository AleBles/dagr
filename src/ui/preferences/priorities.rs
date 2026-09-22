//! Priorities: switch the feature on or off, and add, remove, rename, recolor
//! and reorder the levels. The order of the rows *is* the priority order
//! (top = highest).

use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib;

use super::{icon_button, Prefs};
use crate::settings::Settings;
use crate::task::Priority;
use crate::ui::colors;
use crate::ui::reorder::Reorder;

pub(super) struct Page {
    pub(super) page: adw::PreferencesPage,
    enabled: adw::SwitchRow,
    /// Everything the switch above governs, greyed out while it is off.
    group: adw::PreferencesGroup,
    default_priority: adw::ComboRow,
    list: gtk::ListBox,
    add: gtk::Button,
    /// Drag-and-drop for the list.
    reorder: Reorder,
}

pub(super) fn build(prefs: Weak<Prefs>) -> Page {
    let enabled = adw::SwitchRow::builder()
        .title(crate::tr!("prefs.priorities.switch"))
        .subtitle(crate::tr!("prefs.priorities.switch_sub"))
        .build();
    let switch = adw::PreferencesGroup::new();
    switch.add(&enabled);
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        // The levels are their own thing; let them breathe under the default.
        .margin_top(18)
        .build();
    let add = icon_button("list-add-symbolic", &crate::tr!("prefs.priorities.add"));
    let default_priority = adw::ComboRow::builder()
        .title(crate::tr!("prefs.priorities.default"))
        .subtitle(crate::tr!("prefs.priorities.default_sub"))
        .build();
    let group = adw::PreferencesGroup::builder()
        .description(crate::tr!("prefs.priorities.desc"))
        .header_suffix(&add)
        .build();
    group.add(&default_priority);
    group.add(&list);
    let page = adw::PreferencesPage::builder()
        .title(crate::tr!("prefs.priorities.title"))
        .icon_name("emblem-important-symbolic")
        .build();
    page.add(&switch);
    page.add(&group);
    Page {
        page,
        enabled,
        group,
        default_priority,
        list,
        add,
        reorder: Reorder::new(move |from, to| {
            if let Some(prefs) = prefs.upgrade() {
                reorder_priority(&prefs, from, to);
            }
        }),
    }
}

pub(super) fn connect(prefs: &Rc<Prefs>) {
    let enabled = &prefs.priorities.enabled;
    enabled.connect_active_notify(glib::clone!(
        #[strong]
        prefs,
        move |row| {
            let enabled = row.is_active();
            if enabled == prefs.ctx.settings().priorities_enabled {
                return; // render() syncing the switch, not a user change
            }
            prefs.mutate(move |db| {
                let mut settings = db.settings()?;
                settings.priorities_enabled = enabled;
                db.save_settings(&settings)
            });
        }
    ));
    prefs.priorities.add.connect_clicked(glib::clone!(
        #[strong]
        prefs,
        move |_| add_priority(&prefs)
    ));
    let default_priority = &prefs.priorities.default_priority;
    default_priority.connect_selected_notify(glib::clone!(
        #[strong]
        prefs,
        move |row| {
            if prefs.syncing.get() {
                return;
            }
            // Item 0 is "Lowest", then the priorities in order.
            let chosen = match row.selected() {
                0 => None,
                n => prefs.ctx.priorities().get(n as usize - 1).map(|p| p.id),
            };
            if chosen == prefs.ctx.settings().default_priority_id {
                return;
            }
            prefs.mutate(move |db| {
                let mut settings = db.settings()?;
                settings.default_priority_id = chosen;
                db.save_settings(&settings)
            });
        }
    ));
}

pub(super) fn render(prefs: &Rc<Prefs>, settings: &Settings) {
    let page = &prefs.priorities;
    let priorities = prefs.ctx.priorities();
    page.enabled.set_active(settings.priorities_enabled);
    page.group.set_sensitive(settings.priorities_enabled);

    let mut names = vec![crate::tr!("prefs.priorities.lowest")];
    names.extend(priorities.iter().map(|p| p.name.clone()));
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    page.default_priority
        .set_model(Some(&gtk::StringList::new(&name_refs)));
    let default_index = settings
        .default_priority_id
        .and_then(|id| priorities.iter().position(|p| p.id == id))
        .map_or(0, |i| i + 1);
    page.default_priority.set_selected(default_index as u32);

    page.list.remove_all();
    let count = priorities.len();
    for (index, priority) in priorities.iter().enumerate() {
        page.list.append(&build_row(prefs, priority, index, count));
    }
}

fn build_row(prefs: &Rc<Prefs>, priority: &Priority, index: usize, count: usize) -> adw::EntryRow {
    let id = priority.id;
    let row = adw::EntryRow::builder()
        .title(crate::tr!("prefs.name"))
        .text(&priority.name)
        .show_apply_button(true)
        .build();
    row.connect_apply(glib::clone!(
        #[weak]
        prefs,
        move |row| {
            let name = row.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            prefs.mutate(move |db| db.rename_priority(id, &name));
        }
    ));

    let color_dialog = gtk::ColorDialog::builder()
        .title(crate::tr!("prefs.color"))
        .with_alpha(false)
        .build();
    let color = gtk::ColorDialogButton::builder()
        .dialog(&color_dialog)
        .rgba(&colors::parse(&priority.color))
        .valign(gtk::Align::Center)
        .tooltip_text(crate::tr!("prefs.color"))
        .build();
    color.connect_rgba_notify(glib::clone!(
        #[weak]
        prefs,
        move |button| {
            let hex = colors::to_hex(&button.rgba());
            prefs.mutate(move |db| db.set_priority_color(id, &hex));
        }
    ));
    // `add_prefix` inserts at the front, so add the handle last to make it
    // the leftmost element.
    row.add_prefix(&color);
    let handle = Reorder::handle();
    row.add_prefix(&handle);
    prefs.priorities.reorder.attach(row.upcast_ref(), &handle);

    let up = icon_button("go-up-symbolic", &crate::tr!("prefs.priorities.up"));
    up.set_sensitive(index > 0);
    up.connect_clicked(glib::clone!(
        #[weak]
        prefs,
        move |_| move_priority(&prefs, id, -1)
    ));
    let down = icon_button("go-down-symbolic", &crate::tr!("prefs.priorities.down"));
    down.set_sensitive(index + 1 < count);
    down.connect_clicked(glib::clone!(
        #[weak]
        prefs,
        move |_| move_priority(&prefs, id, 1)
    ));
    let remove = icon_button(
        "user-trash-symbolic",
        &crate::tr!("prefs.priorities.remove"),
    );
    remove.set_sensitive(count > 1);
    remove.connect_clicked(glib::clone!(
        #[weak]
        prefs,
        move |_| prefs.mutate(move |db| db.delete_priority(id))
    ));
    row.add_suffix(&up);
    row.add_suffix(&down);
    row.add_suffix(&remove);
    row
}

fn add_priority(prefs: &Rc<Prefs>) {
    let count = prefs.ctx.priorities().len();
    let color = colors::PALETTE[count % colors::PALETTE.len()];
    prefs.mutate(move |db| {
        db.insert_priority(&crate::tr!("prefs.priorities.new_name"), color)
            .map(drop)
    });
    // Focus the new row so its name can be typed straight away.
    let weak = Rc::downgrade(prefs);
    glib::idle_add_local_once(move || {
        if let Some(prefs) = weak.upgrade() {
            if let Some(last) = prefs.priorities.list.last_child() {
                last.grab_focus();
            }
        }
    });
}

/// Drag-and-drop: move the priority at list index `from` to index `to`.
fn reorder_priority(prefs: &Rc<Prefs>, from: usize, to: usize) {
    let mut ids: Vec<i64> = prefs.ctx.priorities().iter().map(|p| p.id).collect();
    if from >= ids.len() || to >= ids.len() {
        return;
    }
    let id = ids.remove(from);
    ids.insert(to, id);
    prefs.mutate(move |db| db.set_priority_order(&ids));
}

fn move_priority(prefs: &Rc<Prefs>, id: i64, delta: isize) {
    let mut ids: Vec<i64> = prefs.ctx.priorities().iter().map(|p| p.id).collect();
    let Some(from) = ids.iter().position(|&p| p == id) else {
        return;
    };
    let to = from as isize + delta;
    if to < 0 || to as usize >= ids.len() {
        return;
    }
    ids.swap(from, to as usize);
    prefs.mutate(move |db| db.set_priority_order(&ids));
}
