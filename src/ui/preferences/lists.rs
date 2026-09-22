//! Lists: switch the feature on or off, choose where new tasks go and which
//! list the window opens on. The lists themselves are added, renamed, reordered
//! and removed in the sidebar, where they are visible while you work.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::Prefs;
use crate::settings::{OpenOn, Settings};

pub(super) struct Page {
    pub(super) page: adw::PreferencesPage,
    enabled: adw::SwitchRow,
    /// Everything the switch above governs, greyed out while it is off.
    group: adw::PreferencesGroup,
    default_list: adw::ComboRow,
    open_on: adw::ComboRow,
}

pub(super) fn build() -> Page {
    let enabled = adw::SwitchRow::builder()
        .title(crate::tr!("prefs.lists.switch"))
        .subtitle(crate::tr!("prefs.lists.switch_sub"))
        .build();
    let switch = adw::PreferencesGroup::new();
    switch.add(&enabled);

    let default_list = adw::ComboRow::builder()
        .title(crate::tr!("prefs.lists.default"))
        .subtitle(crate::tr!("prefs.lists.default_sub"))
        .build();
    let labels = OpenOn::ALL.map(|o| o.label());
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let open_on = adw::ComboRow::builder()
        .title(crate::tr!("prefs.lists.open_on"))
        .model(&gtk::StringList::new(&refs))
        .build();
    let group = adw::PreferencesGroup::builder()
        .description(crate::tr!("prefs.lists.desc"))
        .build();
    group.add(&default_list);
    group.add(&open_on);

    let page = adw::PreferencesPage::builder()
        .title(crate::tr!("prefs.lists.title"))
        .icon_name("view-list-symbolic")
        .build();
    page.add(&switch);
    page.add(&group);
    Page {
        page,
        enabled,
        group,
        default_list,
        open_on,
    }
}

pub(super) fn connect(prefs: &Rc<Prefs>) {
    let enabled = &prefs.lists.enabled;
    enabled.connect_active_notify(glib::clone!(
        #[strong]
        prefs,
        move |row| {
            let enabled = row.is_active();
            if enabled == prefs.ctx.settings().lists_enabled {
                return; // render() syncing the switch, not a user change
            }
            prefs.mutate(move |db| {
                let mut settings = db.settings()?;
                settings.lists_enabled = enabled;
                db.save_settings(&settings)
            });
        }
    ));
    let default_list = &prefs.lists.default_list;
    default_list.connect_selected_notify(glib::clone!(
        #[strong]
        prefs,
        move |row| {
            if prefs.syncing.get() {
                return;
            }
            let chosen = prefs.ctx.lists().get(row.selected() as usize).map(|l| l.id);
            if chosen.is_none() || chosen == prefs.ctx.settings().default_list_id {
                return;
            }
            prefs.mutate(move |db| {
                let mut settings = db.settings()?;
                settings.default_list_id = chosen;
                db.save_settings(&settings)
            });
        }
    ));
    let open_on = &prefs.lists.open_on;
    open_on.connect_selected_notify(glib::clone!(
        #[strong]
        prefs,
        move |row| {
            if prefs.syncing.get() {
                return;
            }
            let Some(open_on) = OpenOn::ALL.get(row.selected() as usize).copied() else {
                return;
            };
            if open_on == prefs.ctx.settings().open_on {
                return;
            }
            prefs.mutate(move |db| {
                let mut settings = db.settings()?;
                settings.open_on = open_on;
                db.save_settings(&settings)
            });
        }
    ));
}

pub(super) fn render(prefs: &Rc<Prefs>, settings: &Settings) {
    let page = &prefs.lists;
    page.enabled.set_active(settings.lists_enabled);
    page.group.set_sensitive(settings.lists_enabled);

    let lists = prefs.ctx.lists();
    let names: Vec<&str> = lists.iter().map(|l| l.name.as_str()).collect();
    page.default_list
        .set_model(Some(&gtk::StringList::new(&names)));
    let default_index = settings
        .default_list_id
        .and_then(|id| lists.iter().position(|l| l.id == id))
        .unwrap_or(0);
    page.default_list.set_selected(default_index as u32);

    let open_index = OpenOn::ALL
        .iter()
        .position(|o| *o == settings.open_on)
        .unwrap_or(0);
    page.open_on.set_selected(open_index as u32);
}
