//! Dagr — a small, priority-ordered task list.
//!
//! `main.rs` only sets up the application: CSS, app-level actions and the
//! window. Everything the user sees lives in `ui/`, data in `db.rs`.

use adw::prelude::*;
use dagr::{db, serve, ui};
use gtk::{gio, glib};

/// Placeholder reverse-DNS id; rename before publishing anywhere.
const APP_ID: &str = "dev.ables.Dagr";

fn main() -> glib::ExitCode {
    // Pick the mode before anything else: the windowless ones must not touch
    // GTK, so they can run under systemd with no display.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") if args.iter().any(|a| a == "--print-unit") => {
            return report("printing the unit", serve::print_unit())
        }
        Some("serve") => return report("background service", serve::run()),
        Some("status") => return report("status", serve::status()),
        Some("setup") => return report("setup", serve::setup()),
        _ => {}
    }
    // The stdio server is gone; there is one MCP endpoint now, served over
    // HTTP by `dagr serve`. Fail loudly so an old config shows up as a broken
    // server rather than hanging on a pipe that will never answer.
    if args.iter().any(|arg| arg == "--mcp") {
        eprintln!(
            "dagr: --mcp has been removed. The MCP server now runs in the background \
             service.\n      Start it with:  systemctl --user enable --now dagr\n      \
             Then point clients at the url from:  dagr status"
        );
        return glib::ExitCode::FAILURE;
    }

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::default())
        .build();

    app.connect_startup(|app| {
        gtk::Window::set_default_icon_name(APP_ID);
        load_css();
        setup_actions(app);
    });
    app.connect_activate(build_ui);
    app.run()
}

/// Turns a mode's result into an exit code, naming the mode on failure.
fn report(what: &str, result: anyhow::Result<()>) -> glib::ExitCode {
    match result {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("dagr: {what} failed: {err:#}");
            glib::ExitCode::FAILURE
        }
    }
}

fn build_ui(app: &adw::Application) {
    // Launching a second instance just raises the existing window.
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    let db = match db::Db::open() {
        Ok(db) => db,
        Err(err) => {
            eprintln!("tasks: cannot open database: {err:#}");
            std::process::exit(1);
        }
    };

    let window = ui::window::build_window(app, db);
    window.present();
}

fn setup_actions(app: &adw::Application) {
    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| app.quit()
    ));
    app.add_action(&quit);

    let about = gio::SimpleAction::new("about", None);
    about.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| show_about(&app)
    ));
    app.add_action(&about);

    app.set_accels_for_action("app.quit", &["<primary>q"]);
    app.set_accels_for_action("win.focus-entry", &["<primary>n"]);
    app.set_accels_for_action("win.preferences", &["<primary>comma"]);
    app.set_accels_for_action("win.clear-completed", &["<primary><shift>d"]);
}

fn show_about(app: &adw::Application) {
    let about = adw::AboutDialog::builder()
        .application_name("Dagr")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .comments("A simple, priority-ordered task list.\n\nShortcuts: Ctrl+N focus the entry, Ctrl+Shift+D clear completed, Ctrl+, preferences, Ctrl+Q quit.")
        .build();
    about.present(app.active_window().as_ref());
}

/// Loads `style.css` (embedded at compile time) for the whole display.
// `style_context_add_provider_for_display` is marked deprecated in GTK 4.10 but
// is still the only way to install an app-wide CSS provider.
#[allow(deprecated)]
fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    let display = gtk::gdk::Display::default().expect("no display available");
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
