//! The Preferences dialog.
//!
//! Features can be switched on and off; the Priorities group lets you add,
//! remove, rename, recolor and reorder levels — the order of the rows *is* the
//! priority order (top = highest). Every change is written to the database
//! immediately and the main list re-renders.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::db::Db;
use crate::mcp_http::Status;
use crate::serve;
use crate::settings::{Settings, SortOrder};
use crate::task::Priority;
use crate::ui::colors;
use crate::ui::reorder::Reorder;
use crate::ui::window::Ctx;

struct Prefs {
    ctx: Rc<Ctx>,
    /// Drag-and-drop for the priorities list.
    reorder: Reorder,
    /// True while `render` is pushing values into widgets, so their change
    /// signals are not mistaken for user edits.
    syncing: Cell<bool>,
    priorities_switch: adw::SwitchRow,
    sort_order: adw::ComboRow,
    default_priority: adw::ComboRow,
    priorities_group: adw::PreferencesGroup,
    list: gtk::ListBox,
    mcp_switch: adw::SwitchRow,
    mcp_port: adw::SpinRow,
    mcp_status: adw::ActionRow,
    mcp_copy: gtk::Button,
}

pub fn open(ctx: &Rc<Ctx>) {
    let settings = ctx.settings();

    // --- Features: one switch per optional feature ---------------------------
    let priorities_switch = adw::SwitchRow::builder()
        .title("Priorities")
        .subtitle("Color-coded levels that order the list")
        .active(settings.priorities_enabled)
        .build();
    let features = adw::PreferencesGroup::builder().title("Features").build();
    features.add(&priorities_switch);

    // --- List ---------------------------------------------------------------------
    let sort_labels: Vec<&str> = SortOrder::ALL.iter().map(|o| o.label()).collect();
    let sort_order = adw::ComboRow::builder()
        .title("Sort order")
        .subtitle("Completed tasks always go last")
        .model(&gtk::StringList::new(&sort_labels))
        .build();
    let list_group = adw::PreferencesGroup::builder().title("List").build();
    list_group.add(&sort_order);

    // --- Priorities ------------------------------------------------------------
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add priority")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let default_priority = adw::ComboRow::builder()
        .title("Default for new tasks")
        .subtitle("Each leading ! in a title moves one level up from here")
        .build();
    let priorities_group = adw::PreferencesGroup::builder()
        .title("Priorities")
        .description("The top one is the highest. Drag the grip or use the arrows to reorder.")
        .header_suffix(&add)
        .build();
    priorities_group.add(&default_priority);
    priorities_group.add(&list);
    // --- AI access ---------------------------------------------------------------
    let mcp_switch = adw::SwitchRow::builder()
        .title("MCP server")
        .subtitle("Let AI assistants read and change your tasks")
        .active(settings.mcp_http_enabled)
        .build();
    let mcp_port = adw::SpinRow::with_range(f64::from(Settings::MIN_PORT), 65535.0, 1.0);
    mcp_port.set_title("Port");
    mcp_port.set_digits(0);
    mcp_port.set_value(f64::from(settings.mcp_http_port));
    let mcp_copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text("Copy address")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let mcp_status = adw::ActionRow::builder()
        .title("Stopped")
        .subtitle_selectable(true)
        .build();
    mcp_status.add_suffix(&mcp_copy);
    let ai = adw::PreferencesGroup::builder()
        .title("AI access")
        .description(format!(
            "A local HTTP endpoint for assistants, served by Dagr's background \
             service so it keeps working after this window is closed. To start it \
             with your session, run: {}",
            serve::setup_command()
        ))
        .build();
    ai.add(&mcp_switch);
    ai.add(&mcp_port);
    ai.add(&mcp_status);

    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();
    page.add(&features);
    page.add(&list_group);
    page.add(&priorities_group);
    page.add(&ai);
    let dialog = adw::PreferencesDialog::builder()
        .title("Preferences")
        .content_width(520)
        .build();
    dialog.add(&page);

    let prefs = Rc::new_cyclic(|weak: &std::rc::Weak<Prefs>| {
        let weak = weak.clone();
        Prefs {
            ctx: Rc::clone(ctx),
            reorder: Reorder::new(move |from, to| {
                if let Some(prefs) = weak.upgrade() {
                    prefs.reorder_priority(from, to);
                }
            }),
            syncing: Cell::new(false),
            priorities_switch: priorities_switch.clone(),
            sort_order: sort_order.clone(),
            default_priority: default_priority.clone(),
            priorities_group,
            list,
            mcp_switch: mcp_switch.clone(),
            mcp_port: mcp_port.clone(),
            mcp_status,
            mcp_copy: mcp_copy.clone(),
        }
    });
    // These two widgets live as long as the dialog and hold the only strong
    // references, so `Prefs` is dropped together with the dialog.
    add.connect_clicked(glib::clone!(
        #[strong]
        prefs,
        move |_| prefs.add_priority()
    ));
    priorities_switch.connect_active_notify(glib::clone!(
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
    sort_order.connect_selected_notify(glib::clone!(
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
    mcp_switch.connect_active_notify(glib::clone!(
        #[strong]
        prefs,
        move |row| {
            let enabled = row.is_active();
            if enabled == prefs.ctx.settings().mcp_http_enabled {
                return;
            }
            prefs.mutate(move |db| {
                let mut settings = db.settings()?;
                settings.mcp_http_enabled = enabled;
                db.save_settings(&settings)
            });
        }
    ));
    mcp_port.connect_value_notify(glib::clone!(
        #[strong]
        prefs,
        move |row| {
            let port = row.value() as u16;
            if port == prefs.ctx.settings().mcp_http_port {
                return;
            }
            prefs.mutate(move |db| {
                let mut settings = db.settings()?;
                settings.mcp_http_port = port;
                db.save_settings(&settings)
            });
        }
    ));
    mcp_copy.connect_clicked(glib::clone!(
        #[strong]
        prefs,
        move |button| {
            if let Status::Running { url } = prefs.ctx.mcp_status() {
                button.clipboard().set_text(&url);
                button.set_icon_name("object-select-symbolic");
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(1200),
                    glib::clone!(
                        #[weak]
                        button,
                        move || button.set_icon_name("edit-copy-symbolic")
                    ),
                );
            }
        }
    ));

    // Follow changes made elsewhere (MCP server) while the dialog is open.
    ctx.set_refresh_listener(Some(Box::new(glib::clone!(
        #[weak]
        prefs,
        move || prefs.render()
    ))));
    dialog.connect_closed(glib::clone!(
        #[weak]
        ctx,
        move |_| ctx.set_refresh_listener(None)
    ));
    prefs.render();
    dialog.present(Some(&ctx.window));
}

impl Prefs {
    fn render(self: &Rc<Self>) {
        self.syncing.set(true);
        let settings = self.ctx.settings();
        let priorities = self.ctx.priorities();
        let enabled = settings.priorities_enabled;
        self.priorities_switch.set_active(enabled);
        self.priorities_group.set_sensitive(enabled);

        let sort_index = SortOrder::ALL
            .iter()
            .position(|o| *o == settings.sort_order)
            .unwrap_or(0);
        self.sort_order.set_selected(sort_index as u32);

        let mut names = vec!["Lowest".to_string()];
        names.extend(priorities.iter().map(|p| p.name.clone()));
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        self.default_priority
            .set_model(Some(&gtk::StringList::new(&name_refs)));
        let default_index = settings
            .default_priority_id
            .and_then(|id| priorities.iter().position(|p| p.id == id))
            .map_or(0, |i| i + 1);
        self.default_priority.set_selected(default_index as u32);

        self.render_mcp(&settings);
        self.list.remove_all();
        let count = priorities.len();
        for (index, priority) in priorities.iter().enumerate() {
            self.list.append(&self.build_row(priority, index, count));
        }
        self.syncing.set(false);
    }

    fn build_row(
        self: &Rc<Self>,
        priority: &Priority,
        index: usize,
        count: usize,
    ) -> adw::EntryRow {
        let id = priority.id;
        let row = adw::EntryRow::builder()
            .title("Name")
            .text(&priority.name)
            .show_apply_button(true)
            .build();
        row.connect_apply(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            move |row| {
                let name = row.text().trim().to_string();
                if name.is_empty() {
                    return;
                }
                prefs.mutate(move |db| db.rename_priority(id, &name));
            }
        ));

        let color_dialog = gtk::ColorDialog::builder()
            .title("Priority color")
            .with_alpha(false)
            .build();
        let color = gtk::ColorDialogButton::builder()
            .dialog(&color_dialog)
            .rgba(&colors::parse(&priority.color))
            .valign(gtk::Align::Center)
            .tooltip_text("Change color")
            .build();
        color.connect_rgba_notify(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
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
        self.reorder.attach(row.upcast_ref(), &handle);

        let up = icon_button("go-up-symbolic", "Move up (higher priority)");
        up.set_sensitive(index > 0);
        up.connect_clicked(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            move |_| prefs.move_priority(id, -1)
        ));
        let down = icon_button("go-down-symbolic", "Move down (lower priority)");
        down.set_sensitive(index + 1 < count);
        down.connect_clicked(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            move |_| prefs.move_priority(id, 1)
        ));
        let remove = icon_button(
            "user-trash-symbolic",
            "Remove (its tasks move to the next priority)",
        );
        remove.set_sensitive(count > 1);
        remove.connect_clicked(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            move |_| prefs.mutate(move |db| db.delete_priority(id))
        ));
        row.add_suffix(&up);
        row.add_suffix(&down);
        row.add_suffix(&remove);
        row
    }

    fn render_mcp(&self, settings: &Settings) {
        self.mcp_switch.set_active(settings.mcp_http_enabled);
        self.mcp_port.set_value(f64::from(settings.mcp_http_port));
        let (title, subtitle, running) = match self.ctx.mcp_status() {
            // Switched on but nothing listening yet: the window starts the
            // service in the background, so this normally lasts a moment.
            Status::Stopped if settings.mcp_http_enabled => (
                "Starting…",
                "Waiting for the background service".to_string(),
                false,
            ),
            Status::Stopped => ("Stopped", "Turn on above to start".to_string(), false),
            Status::Running { url } => ("Running", url, true),
            Status::Failed(err) => ("Could not start", err, false),
        };
        self.mcp_status.set_title(title);
        self.mcp_status.set_subtitle(&subtitle);
        self.mcp_status.set_tooltip_text(
            running
                .then(|| {
                    format!(
                        "Claude Code: claude mcp add --transport http dagr {url}\n\
                         .mcp.json: {{\"dagr\": {{\"type\": \"http\", \"url\": \"{url}\"}}}}",
                        url = settings.mcp_http_url()
                    )
                })
                .as_deref(),
        );
        self.mcp_copy.set_sensitive(running);
    }

    fn add_priority(self: &Rc<Self>) {
        let count = self.ctx.priorities().len();
        let color = colors::PALETTE[count % colors::PALETTE.len()];
        self.mutate(move |db| db.insert_priority("New priority", color).map(drop));
        // Focus the new row so its name can be typed straight away.
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(prefs) = weak.upgrade() {
                if let Some(last) = prefs.list.last_child() {
                    last.grab_focus();
                }
            }
        });
    }

    /// Drag-and-drop: move the priority at list index `from` to index `to`.
    fn reorder_priority(self: &Rc<Self>, from: usize, to: usize) {
        let mut ids: Vec<i64> = self.ctx.priorities().iter().map(|p| p.id).collect();
        if from >= ids.len() || to >= ids.len() {
            return;
        }
        let id = ids.remove(from);
        ids.insert(to, id);
        self.mutate(move |db| db.set_priority_order(&ids));
    }

    fn move_priority(self: &Rc<Self>, id: i64, delta: isize) {
        let mut ids: Vec<i64> = self.ctx.priorities().iter().map(|p| p.id).collect();
        let Some(from) = ids.iter().position(|&p| p == id) else {
            return;
        };
        let to = from as isize + delta;
        if to < 0 || to as usize >= ids.len() {
            return;
        }
        ids.swap(from, to as usize);
        self.mutate(move |db| db.set_priority_order(&ids));
    }

    /// Runs a database change on the next main-loop iteration. The window
    /// refreshes and, through its refresh listener, re-renders this dialog.
    /// Deferred for the same reason as `Ctx::later`: never destroy the widget
    /// whose signal we are handling.
    fn mutate(self: &Rc<Self>, f: impl FnOnce(&Db) -> anyhow::Result<()> + 'static) {
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(prefs) = weak.upgrade() {
                prefs.ctx.run(f);
            }
        });
    }
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}
