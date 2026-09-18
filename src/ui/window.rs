//! The single application window, plus every state mutation the UI can make.
//!
//! Design: there is no view-model layer. Every change writes to SQLite, reloads
//! the ordered list and rebuilds the `ListBox` from scratch. With a personal
//! task list this is instantaneous and there is nothing to keep in sync.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::db::Db;
use crate::mcp_http;
use crate::serve;
use crate::settings::Settings;
use crate::task::{self, Priority, Task};
use crate::ui::{preferences, task_row};

/// Shared by every widget callback as `Rc<Ctx>`.
///
/// Window-level closures (entry, actions) hold it strongly and keep it alive for
/// the lifetime of the window. Row closures hold it weakly, because rows are
/// destroyed and recreated on every change.
pub struct Ctx {
    pub window: adw::ApplicationWindow,
    db: Db,
    tasks: RefCell<Vec<Task>>,
    priorities: RefCell<Vec<Priority>>,
    settings: RefCell<Settings>,
    /// Called after every render, so an open dialog can follow along.
    refresh_listener: RefCell<Option<Box<dyn Fn()>>>,
    /// Last known state of the MCP endpoint, which the background service
    /// owns. Refreshed on the same tick that watches for outside changes.
    mcp_status: RefCell<mcp_http::Status>,
    /// Last seen SQLite `data_version`, to detect commits by other processes.
    seen_data_version: Cell<i64>,
    entry: gtk::Entry,
    list: gtk::ListBox,
    stack: gtk::Stack,
    clear_button: gtk::Button,
    toasts: adw::ToastOverlay,
}

pub fn build_window(app: &adw::Application, db: Db) -> adw::ApplicationWindow {
    // --- add-task entry -------------------------------------------------
    let entry = gtk::Entry::builder()
        .placeholder_text("Add a task…")
        .secondary_icon_name("list-add-symbolic")
        .secondary_icon_activatable(true)
        .secondary_icon_tooltip_text("Add task")
        .build();

    // --- the list and its empty state -----------------------------------
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .valign(gtk::Align::Start)
        .css_classes(["boxed-list"])
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .child(&list)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let empty = adw::StatusPage::builder()
        .icon_name("checkbox-checked-symbolic")
        .title("No tasks")
        .description("Type above and press Enter to add one")
        .vexpand(true)
        .css_classes(["compact"])
        .build();
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .vexpand(true)
        .build();
    stack.add_named(&scroller, Some("list"));
    stack.add_named(&empty, Some("empty"));

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&entry);
    content.append(&stack);
    let clamp = adw::Clamp::builder()
        .maximum_size(640)
        .tightening_threshold(400)
        .child(&content)
        .build();

    // --- header bar -----------------------------------------------------
    let clear_button = gtk::Button::builder()
        .label("Clear completed")
        .tooltip_text("Remove all done tasks (Ctrl+Shift+D)")
        .action_name("win.clear-completed")
        .css_classes(["flat"])
        .visible(false)
        .build();
    let menu = gio::Menu::new();
    menu.append(Some("_Preferences"), Some("win.preferences"));
    menu.append(Some("_About Dagr"), Some("app.about"));
    menu.append(Some("_Quit"), Some("app.quit"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Main menu")
        .primary(true)
        .build();
    let header = adw::HeaderBar::new();
    header.pack_end(&menu_button);
    header.pack_end(&clear_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&clamp));
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&toolbar));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Dagr")
        .default_width(420)
        .default_height(600)
        .width_request(360)
        .height_request(300)
        .content(&toasts)
        .build();

    let ctx = Rc::new(Ctx {
        window,
        db,
        tasks: RefCell::new(Vec::new()),
        priorities: RefCell::new(Vec::new()),
        settings: RefCell::new(Settings::default()),
        refresh_listener: RefCell::new(None),
        mcp_status: RefCell::new(mcp_http::Status::Stopped),
        seen_data_version: Cell::new(0),
        entry,
        list,
        stack,
        clear_button,
        toasts,
    });
    ctx.wire_up();
    ctx.refresh();
    ctx.window.clone()
}

impl Ctx {
    fn wire_up(self: &Rc<Self>) {
        self.entry.connect_activate(glib::clone!(
            #[strong(rename_to = ctx)]
            self,
            move |entry| {
                ctx.add_task(&entry.text());
                entry.set_text("");
            }
        ));
        // The "+" icon does the same as pressing Enter.
        self.entry.connect_icon_release(|entry, _| {
            entry.activate();
        });

        let focus = gio::SimpleAction::new("focus-entry", None);
        focus.connect_activate(glib::clone!(
            #[strong(rename_to = ctx)]
            self,
            move |_, _| {
                ctx.entry.grab_focus();
            }
        ));
        self.window.add_action(&focus);

        let clear = gio::SimpleAction::new("clear-completed", None);
        clear.connect_activate(glib::clone!(
            #[strong(rename_to = ctx)]
            self,
            move |_, _| ctx.clear_completed()
        ));
        self.window.add_action(&clear);

        let prefs = gio::SimpleAction::new("preferences", None);
        prefs.connect_activate(glib::clone!(
            #[strong(rename_to = ctx)]
            self,
            move |_, _| preferences::open(&ctx)
        ));
        self.window.add_action(&prefs);

        // Start with the cursor in the entry: type + Enter adds a task, zero clicks.
        self.entry.grab_focus();

        // Pick up changes made by the MCP server (a separate process on the
        // same database) within a second.
        self.seen_data_version
            .set(self.db.data_version().unwrap_or(0));
        glib::timeout_add_local(
            Duration::from_secs(1),
            glib::clone!(
                #[weak(rename_to = ctx)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    ctx.poll_external_changes();
                    glib::ControlFlow::Continue
                }
            ),
        );
    }

    /// The MCP endpoint lives in the background service now, so make sure one
    /// is running when the setting asks for it. Nothing is stopped here: the
    /// service is shared, and it outliving this window is the entire point.
    fn ensure_service_running(&self) {
        if !self.settings.borrow().mcp_http_enabled {
            return;
        }
        if let Err(err) = serve::ensure_running() {
            eprintln!("dagr: could not start the background service: {err:#}");
        }
    }

    pub fn mcp_status(&self) -> mcp_http::Status {
        self.mcp_status.borrow().clone()
    }

    fn poll_external_changes(self: &Rc<Self>) {
        let mut stale = false;
        if let Ok(version) = self.db.data_version() {
            stale |= self.seen_data_version.replace(version) != version;
        }
        // The service can start, stop or fail without touching the database.
        let status = serve::mcp_status();
        if *self.mcp_status.borrow() != status {
            *self.mcp_status.borrow_mut() = status;
            stale = true;
        }
        if stale {
            self.refresh();
        }
    }

    // --- mutations (each one: write → reload → render) -------------------

    pub fn add_task(self: &Rc<Self>, raw: &str) {
        // With priorities off, `!` is just text.
        let settings = self.settings.borrow();
        let (bangs, title) = if settings.priorities_enabled {
            task::split_priority_prefix(raw)
        } else {
            (0, raw.trim().to_string())
        };
        if title.is_empty() {
            return;
        }
        let picked = task::pick_priority(
            &self.priorities.borrow(),
            settings.default_priority_id,
            bangs,
        );
        drop(settings);
        let Some(priority_id) = picked else {
            self.report(anyhow::anyhow!("No priorities defined"));
            return;
        };
        self.apply(self.db.insert(&title, priority_id).map(drop));
    }

    pub fn set_done(self: &Rc<Self>, id: i64, done: bool) {
        self.apply(self.db.set_done(id, done));
    }

    pub fn set_title(self: &Rc<Self>, id: i64, title: &str) {
        self.apply(self.db.set_title(id, title));
    }

    pub fn set_priority(self: &Rc<Self>, id: i64, priority_id: i64) {
        self.apply(self.db.set_priority(id, priority_id));
    }

    /// Deletes immediately (no confirmation) and offers Undo in a toast.
    pub fn delete_task(self: &Rc<Self>, task: Task) {
        self.apply(self.db.delete(task.id));
        self.undo_toast("Task deleted", vec![task]);
    }

    pub fn clear_completed(self: &Rc<Self>) {
        match self.db.clear_completed() {
            Ok(removed) if removed.is_empty() => {}
            Ok(removed) => {
                let message = match removed.len() {
                    1 => "1 completed task cleared".to_string(),
                    n => format!("{n} completed tasks cleared"),
                };
                self.refresh();
                self.undo_toast(&message, removed);
            }
            Err(err) => self.report(err),
        }
    }

    /// Runs `f` on the next main-loop iteration with a fresh `Rc<Ctx>`.
    ///
    /// Row widgets call this so that the list is never rebuilt from inside one
    /// of its own signal handlers (which would destroy the emitting widget).
    pub fn later(self: &Rc<Self>, f: impl FnOnce(&Rc<Self>) + 'static) {
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(ctx) = weak.upgrade() {
                f(&ctx);
            }
        });
    }

    /// Runs an arbitrary database change (used by the Preferences dialog),
    /// reports any error and refreshes the list.
    pub fn run(self: &Rc<Self>, f: impl FnOnce(&Db) -> anyhow::Result<()>) {
        self.apply(f(&self.db));
    }

    /// A snapshot of the priorities, highest first.
    pub fn priorities(&self) -> Vec<Priority> {
        self.priorities.borrow().clone()
    }

    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    /// Registers (or clears) the one callback run after each render. Used by
    /// the Preferences dialog to stay in sync with changes from the MCP server.
    pub fn set_refresh_listener(&self, listener: Option<Box<dyn Fn()>>) {
        *self.refresh_listener.borrow_mut() = listener;
    }

    // --- internals -------------------------------------------------------

    fn undo_toast(self: &Rc<Self>, title: &str, tasks: Vec<Task>) {
        let toast = adw::Toast::builder()
            .title(title)
            .button_label("Undo")
            .timeout(5)
            .build();
        toast.connect_button_clicked(glib::clone!(
            #[weak(rename_to = ctx)]
            self,
            move |_| {
                let result = tasks.iter().try_for_each(|task| ctx.db.reinsert(task));
                ctx.apply(result);
            }
        ));
        self.toasts.add_toast(toast);
    }

    fn apply(self: &Rc<Self>, result: anyhow::Result<()>) {
        if let Err(err) = result {
            self.report(err);
        }
        self.refresh();
    }

    fn report(&self, err: anyhow::Error) {
        eprintln!("tasks: {err:#}");
        let text = glib::markup_escape_text(&format!("Error: {err}"));
        self.toasts.add_toast(adw::Toast::new(&text));
    }

    pub fn refresh(self: &Rc<Self>) {
        match self.db.settings() {
            Ok(settings) => *self.settings.borrow_mut() = settings,
            Err(err) => self.report(err),
        }
        match self.db.load_priorities() {
            Ok(priorities) => *self.priorities.borrow_mut() = priorities,
            Err(err) => self.report(err),
        }
        match self.db.load_all() {
            Ok(tasks) => *self.tasks.borrow_mut() = tasks,
            Err(err) => self.report(err),
        }
        self.ensure_service_running();
        self.render();
    }

    fn render(self: &Rc<Self>) {
        {
            self.list.remove_all();
            let tasks = self.tasks.borrow();
            let priorities = self.priorities.borrow();
            let settings = self.settings.borrow();
            for task in tasks.iter() {
                self.list
                    .append(&task_row::build_row(task, &priorities, &settings, self));
            }
            self.entry
                .set_tooltip_text(Some(if settings.priorities_enabled {
                    "Press Enter to add. Each leading ! raises the priority one level above the default."
                } else {
                    "Press Enter to add."
                }));
            let page = if tasks.is_empty() { "empty" } else { "list" };
            self.stack.set_visible_child_name(page);
            self.clear_button.set_visible(tasks.iter().any(|t| t.done));
        }
        self.notify_refresh_listener();
    }

    fn notify_refresh_listener(&self) {
        if let Some(listener) = self.refresh_listener.borrow().as_ref() {
            listener();
        }
    }
}
