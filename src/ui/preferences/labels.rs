//! Labels: switch the feature on or off, and add, rename, recolor and remove
//! them. They are listed alphabetically, so there is no order to choose and no
//! default to pick — a task carries as many as it likes, or none at all.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::{icon_button, Prefs};
use crate::settings::Settings;
use crate::task::Label;
use crate::ui::colors;

/// The name a fresh label is created with, and found again by to focus it.
fn new_label() -> String {
    crate::tr!("prefs.labels.new_name")
}

pub(super) struct Page {
    pub(super) page: adw::PreferencesPage,
    enabled: adw::SwitchRow,
    /// Everything the switch above governs, greyed out while it is off.
    group: adw::PreferencesGroup,
    list: gtk::ListBox,
    /// Shown in place of the list until the first label exists.
    empty: gtk::Label,
    add: gtk::Button,
}

pub(super) fn build() -> Page {
    let enabled = adw::SwitchRow::builder()
        .title(crate::tr!("prefs.labels.switch"))
        .subtitle(crate::tr!("prefs.labels.switch_sub"))
        .build();
    let switch = adw::PreferencesGroup::new();
    switch.add(&enabled);

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let empty = gtk::Label::builder()
        .label(crate::tr!("prefs.labels.empty"))
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    let add = icon_button("list-add-symbolic", &crate::tr!("prefs.labels.add"));
    let group = adw::PreferencesGroup::builder()
        .description(crate::tr!("prefs.labels.desc"))
        .header_suffix(&add)
        .build();
    group.add(&list);
    group.add(&empty);

    let page = adw::PreferencesPage::builder()
        .title(crate::tr!("prefs.labels.title"))
        .icon_name("user-bookmarks-symbolic")
        .build();
    page.add(&switch);
    page.add(&group);
    Page {
        page,
        enabled,
        group,
        list,
        empty,
        add,
    }
}

pub(super) fn connect(prefs: &Rc<Prefs>) {
    let enabled = &prefs.labels.enabled;
    enabled.connect_active_notify(glib::clone!(
        #[strong]
        prefs,
        move |row| {
            let enabled = row.is_active();
            if enabled == prefs.ctx.settings().labels_enabled {
                return; // render() syncing the switch, not a user change
            }
            prefs.mutate(move |db| {
                let mut settings = db.settings()?;
                settings.labels_enabled = enabled;
                db.save_settings(&settings)
            });
        }
    ));
    prefs.labels.add.connect_clicked(glib::clone!(
        #[strong]
        prefs,
        move |_| add_label(&prefs)
    ));
}

pub(super) fn render(prefs: &Rc<Prefs>, settings: &Settings) {
    let page = &prefs.labels;
    let labels = prefs.ctx.labels();
    page.enabled.set_active(settings.labels_enabled);
    page.group.set_sensitive(settings.labels_enabled);

    page.list.remove_all();
    for label in &labels {
        page.list.append(&build_row(prefs, label));
    }
    page.list.set_visible(!labels.is_empty());
    page.empty.set_visible(labels.is_empty());
}

fn build_row(prefs: &Rc<Prefs>, label: &Label) -> adw::EntryRow {
    let id = label.id;
    let row = adw::EntryRow::builder()
        .title(crate::tr!("prefs.name"))
        .text(&label.name)
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
            prefs.mutate(move |db| db.rename_label(id, &name));
        }
    ));

    let color_dialog = gtk::ColorDialog::builder()
        .title(crate::tr!("prefs.color"))
        .with_alpha(false)
        .build();
    let color = gtk::ColorDialogButton::builder()
        .dialog(&color_dialog)
        .rgba(&colors::parse(&label.color))
        .valign(gtk::Align::Center)
        .tooltip_text(crate::tr!("prefs.color"))
        .build();
    color.connect_rgba_notify(glib::clone!(
        #[weak]
        prefs,
        move |button| {
            let hex = colors::to_hex(&button.rgba());
            prefs.mutate(move |db| db.set_label_color(id, &hex));
        }
    ));
    row.add_prefix(&color);

    // Always available: unlike priorities, having no labels at all is fine.
    let remove = icon_button("user-trash-symbolic", &crate::tr!("prefs.labels.remove"));
    remove.connect_clicked(glib::clone!(
        #[weak]
        prefs,
        move |_| prefs.mutate(move |db| db.delete_label(id))
    ));
    row.add_suffix(&remove);
    row
}

fn add_label(prefs: &Rc<Prefs>) {
    let color = colors::PALETTE[prefs.ctx.labels().len() % colors::PALETTE.len()];
    let name = new_label();
    prefs.mutate(move |db| db.insert_label(&name, color).map(drop));
    // Focus the new row so its name can be typed straight away. This idle runs
    // after `mutate`'s, so the list has been rebuilt by then — and since labels
    // are alphabetical the new row is wherever "New label" sorts, not last.
    let weak = Rc::downgrade(prefs);
    glib::idle_add_local_once(move || {
        if let Some(prefs) = weak.upgrade() {
            let index = prefs
                .ctx
                .labels()
                .iter()
                .position(|l| l.name == new_label());
            if let Some(row) = index.and_then(|i| prefs.labels.list.row_at_index(i as i32)) {
                row.grab_focus();
            }
        }
    });
}
