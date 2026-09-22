//! AI access: the MCP endpoint hosted by the background service.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::{icon_button, Prefs};
use crate::mcp_http::Status;
use crate::serve;
use crate::settings::Settings;

pub(super) struct Page {
    pub(super) page: adw::PreferencesPage,
    enabled: adw::SwitchRow,
    port: adw::SpinRow,
    status: adw::ActionRow,
    copy: gtk::Button,
}

pub(super) fn build() -> Page {
    let enabled = adw::SwitchRow::builder()
        .title(crate::tr!("prefs.ai.switch"))
        .subtitle(crate::tr!("prefs.ai.switch_sub"))
        .build();
    let port = adw::SpinRow::with_range(f64::from(Settings::MIN_PORT), 65535.0, 1.0);
    port.set_title(&crate::tr!("prefs.ai.port"));
    port.set_digits(0);
    let copy = icon_button("edit-copy-symbolic", &crate::tr!("prefs.ai.copy"));
    let status = adw::ActionRow::builder()
        .title(crate::tr!("prefs.ai.stopped"))
        .subtitle_selectable(true)
        .build();
    status.add_suffix(&copy);
    let group = adw::PreferencesGroup::builder()
        .description(crate::tr!("prefs.ai.desc", command => serve::setup_command()))
        .build();
    group.add(&enabled);
    group.add(&port);
    group.add(&status);
    let page = adw::PreferencesPage::builder()
        .title(crate::tr!("prefs.ai.title"))
        .icon_name("network-server-symbolic")
        .build();
    page.add(&group);
    Page {
        page,
        enabled,
        port,
        status,
        copy,
    }
}

pub(super) fn connect(prefs: &Rc<Prefs>) {
    prefs.ai.enabled.connect_active_notify(glib::clone!(
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
    prefs.ai.port.connect_value_notify(glib::clone!(
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
    prefs.ai.copy.connect_clicked(glib::clone!(
        #[strong]
        prefs,
        move |button| {
            if let Status::Running { url } = prefs.ctx.mcp_status() {
                button.clipboard().set_text(&url);
                button.set_icon_name("object-select-symbolic");
                glib::timeout_add_local_once(
                    Duration::from_millis(1200),
                    glib::clone!(
                        #[weak]
                        button,
                        move || button.set_icon_name("edit-copy-symbolic")
                    ),
                );
            }
        }
    ));
}

pub(super) fn render(prefs: &Rc<Prefs>, settings: &Settings) {
    let page = &prefs.ai;
    page.enabled.set_active(settings.mcp_http_enabled);
    page.port.set_value(f64::from(settings.mcp_http_port));
    let (title, subtitle, running) = match prefs.ctx.mcp_status() {
        // Switched on but nothing listening yet: the window starts the
        // service in the background, so this normally lasts a moment.
        Status::Stopped if settings.mcp_http_enabled => (
            crate::tr!("prefs.ai.starting"),
            crate::tr!("prefs.ai.starting_sub"),
            false,
        ),
        Status::Stopped => (
            crate::tr!("prefs.ai.stopped"),
            crate::tr!("prefs.ai.stopped_sub"),
            false,
        ),
        Status::Running { url } => (crate::tr!("prefs.ai.running"), url, true),
        Status::Failed(err) => (crate::tr!("prefs.ai.failed"), err, false),
    };
    page.status.set_title(&title);
    page.status.set_subtitle(&subtitle);
    page.status.set_tooltip_text(
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
    page.copy.set_sensitive(running);
}
