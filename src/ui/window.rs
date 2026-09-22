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
use crate::settings::{OpenOn, Settings};
use crate::task::{self, Label, List, Priority, Task};
use crate::ui::reorder::Reorder;
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
    /// Alphabetical, straight from the database. Rows resolve their label ids
    /// against this, so the dots are in the same order everywhere.
    labels: RefCell<Vec<Label>>,
    /// In sidebar order. There is always at least one, even with the feature
    /// switched off.
    lists: RefCell<Vec<List>>,
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
    /// The list sidebar, shown only while lists are switched on.
    split: adw::OverlaySplitView,
    sidebar_toggle: gtk::ToggleButton,
    sidebar_list: gtk::ListBox,
    /// True while `render_sidebar` is selecting a row, so its own selection
    /// signal is not mistaken for a click.
    syncing_sidebar: Cell<bool>,
    /// Drag-and-drop for the sidebar.
    reorder: Reorder,
}

/// One sidebar row: "All tasks" has no id and cannot be renamed, dragged or
/// deleted; a list carries a drag handle, an editable name and a trash button.
fn sidebar_row(name: &str, id: Option<i64>, ctx: &Rc<Ctx>) -> gtk::ListBoxRow {
    let row = gtk::Box::builder().spacing(6).build();
    let Some(id) = id else {
        row.append(&gtk::Image::from_icon_name("view-list-symbolic"));
        row.append(
            &gtk::Label::builder()
                .label(name)
                .hexpand(true)
                .xalign(0.0)
                .build(),
        );
        return gtk::ListBoxRow::builder().child(&row).build();
    };

    let handle = Reorder::handle();
    row.append(&handle);
    let label = gtk::EditableLabel::builder()
        .text(name)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let original = name.to_string();
    label.connect_editing_notify(glib::clone!(
        #[weak]
        ctx,
        move |label| {
            if label.is_editing() {
                return;
            }
            let text = label.text().trim().to_string();
            if text.is_empty() || text == original {
                label.set_text(&original);
                return;
            }
            ctx.later(move |ctx| ctx.rename_list(id, &text));
        }
    ));
    row.append(&label);
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text(crate::tr!("sidebar.delete"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    delete.connect_clicked(glib::clone!(
        #[weak]
        ctx,
        move |_| ctx.later(move |ctx| ctx.confirm_delete_list(id))
    ));
    row.append(&delete);

    let row = gtk::ListBoxRow::builder().child(&row).build();
    ctx.reorder.attach(row.upcast_ref(), &handle);
    row
}

pub fn build_window(app: &adw::Application, db: Db) -> adw::ApplicationWindow {
    // Read once, before the window exists: changing the size in Preferences
    // takes effect the next time Dagr starts.
    let startup = db.settings().unwrap_or_default();
    // --- add-task entry -------------------------------------------------
    let entry = gtk::Entry::builder()
        .placeholder_text(crate::tr!("entry.placeholder"))
        .secondary_icon_name("list-add-symbolic")
        .secondary_icon_activatable(true)
        .secondary_icon_tooltip_text(crate::tr!("entry.add"))
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
        .title(crate::tr!("empty.title"))
        .description(crate::tr!("empty.body"))
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
        .label(crate::tr!("header.clear"))
        .tooltip_text(crate::tr!("header.clear_tooltip"))
        .action_name("win.clear-completed")
        .css_classes(["flat"])
        .visible(false)
        .build();
    let menu = gio::Menu::new();
    menu.append(
        Some(&crate::tr!("menu.preferences")),
        Some("win.preferences"),
    );
    menu.append(Some(&crate::tr!("menu.about")), Some("app.about"));
    menu.append(Some(&crate::tr!("menu.quit")), Some("app.quit"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text(crate::tr!("header.menu"))
        .primary(true)
        .build();
    let sidebar_toggle = gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text(crate::tr!("header.sidebar"))
        // Hidden until the feature is on, which is also how `render_sidebar`
        // notices the moment it is switched on.
        .visible(false)
        .build();
    let header = adw::HeaderBar::new();
    header.pack_start(&sidebar_toggle);
    header.pack_end(&menu_button);
    header.pack_end(&clear_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&clamp));

    // --- the list sidebar -------------------------------------------------
    let sidebar_list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    let add_list = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text(crate::tr!("sidebar.new"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let sidebar_header = adw::HeaderBar::builder()
        .show_end_title_buttons(false)
        .build();
    sidebar_header.set_title_widget(Some(&adw::WindowTitle::new(
        &crate::tr!("sidebar.title"),
        "",
    )));
    sidebar_header.pack_end(&add_list);
    let sidebar_toolbar = adw::ToolbarView::new();
    sidebar_toolbar.add_top_bar(&sidebar_header);
    sidebar_toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&sidebar_list)
            .build(),
    ));
    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar_toolbar)
        .content(&toolbar)
        .min_sidebar_width(180.0)
        .max_sidebar_width(240.0)
        .build();

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&split));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Dagr")
        .default_width(startup.window_width)
        .default_height(startup.window_height)
        .width_request(Settings::MIN_WINDOW_WIDTH)
        .height_request(Settings::MIN_WINDOW_HEIGHT)
        .content(&toasts)
        .build();

    let ctx = Rc::new_cyclic(|weak: &std::rc::Weak<Ctx>| Ctx {
        window,
        db,
        tasks: RefCell::new(Vec::new()),
        priorities: RefCell::new(Vec::new()),
        labels: RefCell::new(Vec::new()),
        lists: RefCell::new(Vec::new()),
        settings: RefCell::new(Settings::default()),
        refresh_listener: RefCell::new(None),
        mcp_status: RefCell::new(mcp_http::Status::Stopped),
        seen_data_version: Cell::new(0),
        entry,
        list,
        stack,
        clear_button,
        toasts,
        split: split.clone(),
        sidebar_toggle: sidebar_toggle.clone(),
        sidebar_list: sidebar_list.clone(),
        syncing_sidebar: Cell::new(false),
        reorder: Reorder::new({
            let weak = weak.clone();
            move |from, to| {
                if let Some(ctx) = weak.upgrade() {
                    ctx.reorder_lists(from, to);
                }
            }
        }),
    });
    // The sidebar is bound both ways: the button shows it, and collapsing it
    // by dragging the handle unpresses the button.
    sidebar_toggle
        .bind_property("active", &split, "show-sidebar")
        .bidirectional()
        .sync_create()
        .build();
    add_list.connect_clicked(glib::clone!(
        #[strong(rename_to = ctx)]
        ctx,
        move |_| ctx.add_list()
    ));
    sidebar_list.connect_row_selected(glib::clone!(
        #[strong(rename_to = ctx)]
        ctx,
        move |_, row| {
            if ctx.syncing_sidebar.get() {
                return;
            }
            let Some(row) = row else { return };
            // Row 0 is "All tasks"; the rest are the lists in order.
            let chosen = match row.index() {
                0 => None,
                n => ctx.lists.borrow().get(n as usize - 1).map(|l| l.id),
            };
            ctx.later(move |ctx| ctx.show_list(chosen));
        }
    ));
    ctx.wire_up();
    ctx.apply_open_on();
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
        // A feature that is switched off does not read its marker: `!` and `#`
        // are only prefixes while priorities and labels are on.
        let settings = self.settings.borrow();
        let typed = task::split_prefixes(
            raw,
            settings.priorities_enabled,
            settings.labels_enabled,
            settings.lists_enabled,
        );
        let ignored = task::ignored_markers(
            raw,
            settings.priorities_enabled,
            settings.labels_enabled,
            settings.lists_enabled,
        );
        if typed.title.is_empty() {
            return;
        }
        let picked = task::pick_priority(
            &self.priorities.borrow(),
            settings.default_priority_id,
            typed.bangs,
        );
        drop(settings);
        let Some(priority_id) = picked else {
            self.report(anyhow::anyhow!(crate::tr!("toast.no_priorities")));
            return;
        };
        // A typed `@list` that is no list leaves the task where it would have
        // gone anyway: typing a name never creates a list.
        let mut unknown_list = None;
        let list_id = match &typed.list {
            Some(name) => match self.db.list_by_name(name) {
                Ok(Some(list)) => list.id,
                Ok(None) => {
                    unknown_list = Some(name.clone());
                    self.showing_list()
                }
                Err(err) => {
                    self.report(err);
                    return;
                }
            },
            None => self.showing_list(),
        };
        let result = self
            .db
            .insert_with_labels(&typed.title, priority_id, list_id, &typed.tags);
        let added = result.is_ok();
        self.apply(result.map(drop));
        if added {
            if let Some(name) = unknown_list {
                self.note_unknown_list(&name, list_id);
            }
            self.note_ignored_markers(ignored);
        }
    }

    /// The list a new task goes to: the one showing, or the default when the
    /// window is showing everything or lists are off.
    fn showing_list(&self) -> i64 {
        let showing = {
            let settings = self.settings.borrow();
            settings
                .lists_enabled
                .then_some(settings.current_list_id)
                .flatten()
        };
        showing
            .or_else(|| self.db.default_list().ok())
            .or_else(|| self.lists.borrow().first().map(|l| l.id))
            .unwrap_or_default()
    }

    /// Says where a task went when the `@list` typed for it does not exist,
    /// and offers to make that list after all.
    fn note_unknown_list(self: &Rc<Self>, name: &str, went_to: i64) {
        let landed = self
            .lists
            .borrow()
            .iter()
            .find(|l| l.id == went_to)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| crate::tr!("toast.default_list"));
        let toast = adw::Toast::builder()
            .title(glib::markup_escape_text(&crate::tr!(
                "toast.unknown_list",
                name => name,
                list => landed
            )))
            .button_label(crate::tr!("toast.create_list"))
            .timeout(6)
            .build();
        let name = name.to_string();
        toast.connect_button_clicked(glib::clone!(
            #[weak(rename_to = ctx)]
            self,
            move |_| {
                let name = name.clone();
                ctx.later(move |ctx| ctx.create_and_move_latest(&name));
            }
        ));
        self.toasts.add_toast(toast);
    }

    /// The "Create list" button on that toast: make the list, and move the
    /// task that just missed it into it.
    fn create_and_move_latest(self: &Rc<Self>, name: &str) {
        let newest = self.tasks.borrow().iter().map(|t| t.id).max();
        let result = self.db.insert_list(name).and_then(|list| match newest {
            Some(id) => self.db.set_list(id, list.id),
            None => Ok(()),
        });
        self.apply(result);
    }

    // --- lists -----------------------------------------------------------

    /// Settles which list to show before the first render, following the
    /// "open on" preference. Everything after this just reads the setting.
    fn apply_open_on(self: &Rc<Self>) {
        let Ok(mut settings) = self.db.settings() else {
            return;
        };
        if !settings.lists_enabled {
            return;
        }
        let wanted = match settings.open_on {
            OpenOn::LastUsed => settings.current_list_id,
            OpenOn::DefaultList => self.db.default_list().ok(),
            OpenOn::AllTasks => None,
        };
        if wanted != settings.current_list_id {
            settings.current_list_id = wanted;
            if let Err(err) = self.db.save_settings(&settings) {
                self.report(err);
            }
        }
    }

    /// Shows one list, or everything when given `None`. Stored, so the window
    /// comes back to it and a second window agrees.
    fn show_list(self: &Rc<Self>, list_id: Option<i64>) {
        if self.settings.borrow().current_list_id == list_id {
            return;
        }
        self.apply(self.db.settings().and_then(|mut settings| {
            settings.current_list_id = list_id;
            self.db.save_settings(&settings)
        }));
    }

    fn add_list(self: &Rc<Self>) {
        let taken = self.lists.borrow().len();
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    crate::tr!("sidebar.new_name")
                } else {
                    crate::tr!("sidebar.new_name_numbered", number => n)
                }
            })
            .find(|name| {
                self.lists
                    .borrow()
                    .iter()
                    .all(|l| !l.name.eq_ignore_ascii_case(name))
            })
            .unwrap_or_else(|| crate::tr!("sidebar.new_name_numbered", number => taken));
        self.apply(self.db.insert_list(&name).map(drop));
    }

    pub fn rename_list(self: &Rc<Self>, id: i64, name: &str) {
        self.apply(self.db.rename_list(id, name));
    }

    /// Drag-and-drop in the sidebar: the rows are the lists, offset by the
    /// "All tasks" row above them.
    fn reorder_lists(self: &Rc<Self>, from: usize, to: usize) {
        let mut ids: Vec<i64> = self.lists.borrow().iter().map(|l| l.id).collect();
        let (Some(from), Some(to)) = (from.checked_sub(1), to.checked_sub(1)) else {
            return; // the "All tasks" row does not move
        };
        if from >= ids.len() || to >= ids.len() {
            return;
        }
        let id = ids.remove(from);
        ids.insert(to, id);
        self.apply(self.db.set_list_order(&ids));
    }

    /// Says so when a `!` or a `#tag` was filed as plain text because its
    /// feature is switched off, with a way straight to the switch.
    fn note_ignored_markers(self: &Rc<Self>, ignored: task::Ignored) {
        if !ignored.any() {
            return;
        }
        // One toast naming every marker that was left as text, opening on the
        // page of the first switch to flip.
        let mut markers = Vec::new();
        let mut page = preferences::Page::Settings;
        if ignored.priorities {
            markers.push("!");
            page = preferences::Page::Priorities;
        }
        if ignored.labels {
            markers.push("#tag");
            if !ignored.priorities {
                page = preferences::Page::Labels;
            }
        }
        if ignored.lists {
            markers.push("@list");
        }
        let joiner = format!(" {} ", crate::tr!("toast.markers_and"));
        let title = crate::tr!("toast.markers_off", markers => markers.join(&joiner));
        let toast = adw::Toast::builder()
            .title(&title)
            .button_label(crate::tr!("toast.settings"))
            .timeout(5)
            .build();
        toast.connect_button_clicked(glib::clone!(
            #[weak(rename_to = ctx)]
            self,
            move |_| preferences::open_at(&ctx, page)
        ));
        self.toasts.add_toast(toast);
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

    /// Replaces every label on a task, which is what the row picker hands us.
    pub fn set_task_labels(self: &Rc<Self>, id: i64, label_ids: &[i64]) {
        self.apply(self.db.set_task_labels(id, label_ids));
    }

    /// Deletes immediately (no confirmation) and offers Undo in a toast.
    pub fn delete_task(self: &Rc<Self>, task: Task) {
        self.apply(self.db.delete(task.id));
        self.undo_toast(&crate::tr!("toast.task_deleted"), vec![task]);
    }

    pub fn clear_completed(self: &Rc<Self>) {
        match self.db.clear_completed() {
            Ok(removed) if removed.is_empty() => {}
            Ok(removed) => {
                let message = match removed.len() {
                    1 => crate::tr!("toast.cleared_one"),
                    n => crate::tr!("toast.cleared_many", count => n),
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

    /// A snapshot of the labels, alphabetical.
    pub fn labels(&self) -> Vec<Label> {
        self.labels.borrow().clone()
    }

    /// A snapshot of the lists, in sidebar order.
    pub fn lists(&self) -> Vec<List> {
        self.lists.borrow().clone()
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
            .button_label(crate::tr!("toast.undo"))
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
        let text = glib::markup_escape_text(&crate::tr!("toast.error", message => err));
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
        match self.db.load_labels() {
            Ok(labels) => *self.labels.borrow_mut() = labels,
            Err(err) => self.report(err),
        }
        match self.db.load_lists() {
            Ok(lists) => *self.lists.borrow_mut() = lists,
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
            let labels = self.labels.borrow();
            let settings = self.settings.borrow();
            for task in tasks.iter() {
                self.list.append(&task_row::build_row(
                    task,
                    &priorities,
                    &labels,
                    &settings,
                    self,
                ));
            }
            // Built from a sentence per feature, so every combination of
            // switches reads properly and each language needs four short
            // strings rather than one per combination.
            let mut hint = vec![crate::tr!("entry.hint.base")];
            if settings.priorities_enabled {
                hint.push(crate::tr!("entry.hint.priorities"));
            }
            if settings.labels_enabled {
                hint.push(crate::tr!("entry.hint.labels"));
            }
            if settings.lists_enabled {
                hint.push(crate::tr!("entry.hint.lists"));
            }
            self.entry.set_tooltip_text(Some(&hint.join(" ")));
            let page = if tasks.is_empty() { "empty" } else { "list" };
            self.stack.set_visible_child_name(page);
            self.clear_button.set_visible(tasks.iter().any(|t| t.done));
        }
        {
            let settings = self.settings.borrow();
            self.render_sidebar(&settings);
        }
        self.notify_refresh_listener();
    }

    /// The sidebar: "All tasks", then one row per list. Hidden altogether
    /// while the feature is off, so the window looks exactly as it did.
    fn render_sidebar(self: &Rc<Self>, settings: &Settings) {
        let on = settings.lists_enabled;
        let was_off = !self.sidebar_toggle.is_visible();
        self.sidebar_toggle.set_visible(on);
        if !on {
            self.split.set_show_sidebar(false);
            return;
        }
        if was_off {
            // Just switched on (or just started): show the sidebar once. After
            // that it is the toggle button's business, not ours.
            self.split.set_show_sidebar(true);
        }
        self.syncing_sidebar.set(true);
        self.sidebar_list.remove_all();
        self.sidebar_list
            .append(&sidebar_row(&crate::tr!("sidebar.all"), None, self));
        let lists = self.lists.borrow();
        for list in lists.iter() {
            self.sidebar_list
                .append(&sidebar_row(&list.name, Some(list.id), self));
        }
        let index = match settings.current_list_id {
            None => 0,
            Some(id) => lists
                .iter()
                .position(|l| l.id == id)
                .map_or(0, |i| i as i32 + 1),
        };
        self.sidebar_list
            .select_row(self.sidebar_list.row_at_index(index).as_ref());
        self.syncing_sidebar.set(false);
    }

    /// Asks before deleting a list that holds tasks, because they move rather
    /// than disappear and that is worth saying out loud.
    fn confirm_delete_list(self: &Rc<Self>, id: i64) {
        let Some(list) = self.lists.borrow().iter().find(|l| l.id == id).cloned() else {
            return;
        };
        if self.lists.borrow().len() <= 1 {
            self.toasts
                .add_toast(adw::Toast::new(&crate::tr!("toast.last_list")));
            return;
        }
        let count = self.db.count_in_list(id).unwrap_or(0);
        let fallback = self
            .db
            .default_list()
            .ok()
            .filter(|f| *f != id)
            .or_else(|| {
                self.lists
                    .borrow()
                    .iter()
                    .find(|l| l.id != id)
                    .map(|l| l.id)
            });
        let Some(fallback) = fallback else { return };
        let target = self
            .lists
            .borrow()
            .iter()
            .find(|l| l.id == fallback)
            .map(|l| l.name.clone())
            .unwrap_or_default();
        let body = match count {
            0 => crate::tr!("dialog.delete_list.empty", name => list.name),
            1 => crate::tr!("dialog.delete_list.one", list => target),
            n => crate::tr!("dialog.delete_list.many", count => n, list => target),
        };
        let dialog = adw::AlertDialog::new(
            Some(&crate::tr!("dialog.delete_list.title", name => list.name)),
            Some(&body),
        );
        dialog.add_response("cancel", &crate::tr!("dialog.cancel"));
        dialog.add_response("delete", &crate::tr!("dialog.delete"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = ctx)]
                self,
                move |_, response| {
                    if response == "delete" {
                        ctx.delete_list(id, fallback);
                    }
                }
            ),
        );
        dialog.present(Some(&self.window));
    }

    fn delete_list(self: &Rc<Self>, id: i64, fallback: i64) {
        let result = self.db.delete_list(id, fallback).and_then(|()| {
            let mut settings = self.db.settings()?;
            let mut touched = false;
            if settings.default_list_id == Some(id) {
                settings.default_list_id = None;
                touched = true;
            }
            if settings.current_list_id == Some(id) {
                settings.current_list_id = Some(fallback);
                touched = true;
            }
            if touched {
                self.db.save_settings(&settings)?;
            }
            Ok(())
        });
        self.apply(result);
    }

    fn notify_refresh_listener(&self) {
        if let Some(listener) = self.refresh_listener.borrow().as_ref() {
            listener();
        }
    }
}
