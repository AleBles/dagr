//! Settings: the odds and ends that belong to no feature of their own — how
//! the list is ordered, and how big the window opens.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::Prefs;
use crate::settings::{Language, Settings, SortOrder};

pub(super) struct Page {
    pub(super) page: adw::PreferencesPage,
    sort_order: adw::ComboRow,
    date_first: adw::SwitchRow,
    language: adw::ComboRow,
    width: adw::SpinRow,
    height: adw::SpinRow,
}

pub(super) fn build() -> Page {
    let labels = SortOrder::ALL.map(|o| o.label());
    let sort_order = adw::ComboRow::builder()
        .title(crate::tr!("prefs.settings.sort_order"))
        .subtitle(crate::tr!("prefs.settings.sort_order_sub"))
        .model(&string_list(&labels))
        .build();
    let date_first = adw::SwitchRow::builder()
        .title(crate::tr!("prefs.settings.date_first"))
        .subtitle(crate::tr!("prefs.settings.date_first_sub"))
        .build();
    let list = adw::PreferencesGroup::builder()
        .title(crate::tr!("prefs.settings.ordering"))
        .build();
    list.add(&sort_order);
    list.add(&date_first);

    let language_labels = Language::ALL.map(|l| l.label());
    let language = adw::ComboRow::builder()
        .title(crate::tr!("prefs.settings.language"))
        .model(&string_list(&language_labels))
        .build();
    let languages = adw::PreferencesGroup::builder()
        .title(crate::tr!("prefs.settings.language"))
        .description(crate::tr!("prefs.settings.language_desc"))
        .build();
    languages.add(&language);

    let width = size_row(
        &crate::tr!("prefs.settings.width"),
        Settings::MIN_WINDOW_WIDTH,
    );
    let height = size_row(
        &crate::tr!("prefs.settings.height"),
        Settings::MIN_WINDOW_HEIGHT,
    );
    let window = adw::PreferencesGroup::builder()
        .title(crate::tr!("prefs.settings.window"))
        .description(crate::tr!("prefs.settings.window_desc"))
        .build();
    window.add(&width);
    window.add(&height);

    let page = adw::PreferencesPage::builder()
        .title(crate::tr!("prefs.settings.title"))
        .icon_name("preferences-system-symbolic")
        .build();
    page.add(&list);
    page.add(&languages);
    page.add(&window);
    Page {
        page,
        sort_order,
        date_first,
        language,
        width,
        height,
    }
}

pub(super) fn connect(prefs: &Rc<Prefs>) {
    prefs
        .settings
        .sort_order
        .connect_selected_notify(glib::clone!(
            #[strong]
            prefs,
            move |row| {
                if prefs.syncing.get() {
                    return;
                }
                let Some(order) = SortOrder::ALL.get(row.selected() as usize).copied() else {
                    return;
                };
                if order == prefs.ctx.settings().sort_order {
                    return;
                }
                prefs.mutate(move |db| {
                    let mut settings = db.settings()?;
                    settings.sort_order = order;
                    db.save_settings(&settings)
                });
            }
        ));
    prefs
        .settings
        .date_first
        .connect_active_notify(glib::clone!(
            #[strong]
            prefs,
            move |row| {
                let enabled = row.is_active();
                if enabled == prefs.ctx.settings().date_first {
                    return; // render() syncing the switch, not a user change
                }
                prefs.mutate(move |db| {
                    let mut settings = db.settings()?;
                    settings.date_first = enabled;
                    db.save_settings(&settings)
                });
            }
        ));
    prefs
        .settings
        .language
        .connect_selected_notify(glib::clone!(
            #[strong]
            prefs,
            move |row| {
                if prefs.syncing.get() {
                    return;
                }
                let Some(language) = Language::ALL.get(row.selected() as usize).copied() else {
                    return;
                };
                if language == prefs.ctx.settings().language {
                    return;
                }
                prefs.mutate(move |db| {
                    let mut settings = db.settings()?;
                    settings.language = language;
                    db.save_settings(&settings)
                });
            }
        ));
    prefs.settings.width.connect_value_notify(glib::clone!(
        #[strong]
        prefs,
        move |row| {
            let width = row.value() as i32;
            if width == prefs.ctx.settings().window_width {
                return;
            }
            prefs.mutate(move |db| {
                let mut settings = db.settings()?;
                settings.window_width = width;
                db.save_settings(&settings)
            });
        }
    ));
    prefs.settings.height.connect_value_notify(glib::clone!(
        #[strong]
        prefs,
        move |row| {
            let height = row.value() as i32;
            if height == prefs.ctx.settings().window_height {
                return;
            }
            prefs.mutate(move |db| {
                let mut settings = db.settings()?;
                settings.window_height = height;
                db.save_settings(&settings)
            });
        }
    ));
}

pub(super) fn render(prefs: &Rc<Prefs>, settings: &Settings) {
    let page = &prefs.settings;
    let index = SortOrder::ALL
        .iter()
        .position(|o| *o == settings.sort_order)
        .unwrap_or(0);
    page.sort_order.set_selected(index as u32);
    page.date_first.set_active(settings.date_first);
    let language_index = Language::ALL
        .iter()
        .position(|l| *l == settings.language)
        .unwrap_or(0);
    page.language.set_selected(language_index as u32);
    page.width.set_value(f64::from(settings.window_width));
    page.height.set_value(f64::from(settings.window_height));
}

/// A `StringList` from owned labels, which is what translated text gives us.
fn string_list(labels: &[String]) -> gtk::StringList {
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    gtk::StringList::new(&refs)
}

fn size_row(title: &str, min: i32) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(f64::from(min), f64::from(Settings::MAX_WINDOW_SIZE), 10.0);
    row.set_title(title);
    row.set_digits(0);
    row
}
