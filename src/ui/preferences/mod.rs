//! The Preferences dialog: a sidebar on the left lists the pages, and the
//! selected one fills the rest of the dialog.
//!
//! One module per page, each offering the same three functions: `build` makes
//! the widgets, `connect` wires their signals, `render` pushes the stored
//! settings back into them. `Prefs` is the single shared object they all reach
//! through. Every handler writes to the database via `mutate`; the window
//! refreshes, and its refresh listener re-renders this dialog, so no widget
//! ever has to update another one by hand.

mod ai;
mod labels;
mod lists;
mod priorities;
mod settings;

use std::cell::Cell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib;

use crate::db::Db;
use crate::ui::window::Ctx;

struct Prefs {
    ctx: Rc<Ctx>,
    /// True while `render` is pushing values into widgets, so their change
    /// signals are not mistaken for user edits.
    syncing: Cell<bool>,
    settings: settings::Page,
    priorities: priorities::Page,
    labels: labels::Page,
    lists: lists::Page,
    ai: ai::Page,
}

/// Which page Preferences opens on. Keep in step with the `pages` array in
/// `open_at`, which is the order of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Settings,
    Priorities,
    Labels,
    Lists,
    Ai,
}

impl Page {
    fn index(self) -> i32 {
        match self {
            Page::Settings => 0,
            Page::Priorities => 1,
            Page::Labels => 2,
            Page::Lists => 3,
            Page::Ai => 4,
        }
    }
}

pub fn open(ctx: &Rc<Ctx>) {
    open_at(ctx, Page::Settings);
}

/// Opens Preferences showing one particular page, which is how a toast sends
/// someone straight to the switch it is talking about.
pub fn open_at(ctx: &Rc<Ctx>, page: Page) {
    // `new_cyclic` so the priorities page can hand its drag-and-drop helper a
    // weak reference back to the `Prefs` it lives in.
    let prefs = Rc::new_cyclic(|weak: &Weak<Prefs>| Prefs {
        ctx: Rc::clone(ctx),
        syncing: Cell::new(false),
        settings: settings::build(),
        priorities: priorities::build(weak.clone()),
        labels: labels::build(),
        lists: lists::build(),
        ai: ai::build(),
    });

    // The sidebar is built from the pages themselves, so a page cannot appear
    // in one half of the dialog and be missing from the other.
    let pages = [
        prefs.settings.page.clone(),
        prefs.priorities.page.clone(),
        prefs.labels.page.clone(),
        prefs.lists.page.clone(),
        prefs.ai.page.clone(),
    ];
    let stack = gtk::Stack::new();
    let sidebar = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    for (index, page) in pages.iter().enumerate() {
        stack.add_child(page);
        sidebar.append(&sidebar_row(page, index));
    }

    let content = adw::NavigationPage::builder()
        .title(pages[0].title())
        .child(&toolbar(&stack))
        .build();
    sidebar.connect_row_selected(glib::clone!(
        #[weak]
        stack,
        #[weak]
        content,
        #[strong]
        pages,
        move |_, row| {
            let Some(page) = row.and_then(|row| pages.get(row.index() as usize)) else {
                return;
            };
            stack.set_visible_child(page);
            content.set_title(&page.title());
        }
    ));
    sidebar.select_row(sidebar.row_at_index(page.index()).as_ref());

    // Ctrl+1 … Ctrl+N jump to the pages in sidebar order, from the same array
    // the sidebar itself is built from, so the two cannot drift apart. They
    // select the row rather than setting the stack child, which leaves the
    // handler above in charge of the stack, the header title and the
    // highlight. Capture phase, so an entry being edited cannot swallow them.
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_scope(gtk::ShortcutScope::Local);
    shortcuts.set_propagation_phase(gtk::PropagationPhase::Capture);
    for index in 0..pages.len().min(9) {
        let Some(trigger) = gtk::ShortcutTrigger::parse_string(&format!("<primary>{}", index + 1))
        else {
            continue;
        };
        let action = gtk::CallbackAction::new(glib::clone!(
            #[weak]
            sidebar,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _| {
                sidebar.select_row(sidebar.row_at_index(index as i32).as_ref());
                glib::Propagation::Stop
            }
        ));
        shortcuts.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action)));
    }

    let sidebar_page = adw::NavigationPage::builder()
        .title(crate::tr!("prefs.title"))
        .child(&toolbar(&scrolled(&sidebar)))
        .build();
    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&content)
        .min_sidebar_width(180.0)
        .max_sidebar_width(220.0)
        .build();
    let dialog = adw::Dialog::builder()
        .title(crate::tr!("prefs.title"))
        .content_width(720)
        .content_height(560)
        .child(&split)
        .build();
    dialog.add_controller(shortcuts);

    // Every handler holds a strong reference to `Prefs`, and `Prefs` holds the
    // widgets: the cycle is broken when the dialog drops its widget tree.
    settings::connect(&prefs);
    priorities::connect(&prefs);
    labels::connect(&prefs);
    lists::connect(&prefs);
    ai::connect(&prefs);

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

/// A page's own title and icon make its sidebar row; its position makes the
/// shortcut named in the tooltip.
fn sidebar_row(page: &adw::PreferencesPage, index: usize) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(12).build();
    if let Some(icon) = page.icon_name() {
        content.append(&gtk::Image::from_icon_name(&icon));
    }
    content.append(&gtk::Label::new(Some(&page.title())));
    let row = gtk::ListBoxRow::builder().child(&content).build();
    if index < 9 {
        row.set_tooltip_text(Some(&format!("Ctrl+{}", index + 1)));
    }
    row
}

/// Both halves of the split view need their own header bar, so both are
/// wrapped in a toolbar view.
fn toolbar(content: &impl IsA<gtk::Widget>) -> adw::ToolbarView {
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(content));
    view
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(child)
        .build()
}

impl Prefs {
    fn render(self: &Rc<Self>) {
        self.syncing.set(true);
        let settings = self.ctx.settings();
        settings::render(self, &settings);
        priorities::render(self, &settings);
        labels::render(self, &settings);
        lists::render(self, &settings);
        ai::render(self, &settings);
        self.syncing.set(false);
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

/// The small flat buttons used in the priority rows and group headers.
fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}
