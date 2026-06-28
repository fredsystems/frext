//! The frext eframe application: tab bar, editor surface, and the wiring
//! that keeps swap files and the session index up to date.

use eframe::egui;

use crate::{persistence::Store, tab::Tab};

/// The top-level application state.
pub struct FrextApp {
    store: Store,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
}

impl FrextApp {
    /// Build the app, restoring the previous session from the store.
    #[must_use]
    pub fn new(store: Store) -> Self {
        let (mut tabs, active, mut next_id) = store.load().unwrap_or_else(|err| {
            log::error!("failed to load session, starting fresh: {err}");
            (Vec::new(), 0, 0)
        });

        // Always guarantee at least one tab to edit.
        if tabs.is_empty() {
            tabs.push(Tab::new_untitled(next_id));
            next_id += 1;
        }

        Self {
            store,
            tabs,
            active,
            next_id,
        }
    }

    /// Hand out a fresh, unique tab id.
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Persist the session index, logging (but not propagating) failures so
    /// a transient write error never crashes the editor.
    fn persist_session(&self) {
        if let Err(err) = self
            .store
            .save_session(&self.tabs, self.active, self.next_id)
        {
            log::error!("failed to save session index: {err}");
        }
    }

    /// Write the active tab's buffer to its swap file.
    fn persist_active_swap(&self) {
        if let Some(tab) = self.tabs.get(self.active) {
            if let Err(err) = self.store.save_swap(tab) {
                log::error!("failed to write swap file for tab {}: {err}", tab.id);
            }
        }
    }

    /// Open a new, empty, untitled tab and focus it.
    fn new_tab(&mut self) {
        let id = self.alloc_id();
        self.tabs.push(Tab::new_untitled(id));
        self.active = self.tabs.len() - 1;
        self.persist_active_swap();
        self.persist_session();
    }

    /// Close the tab at `index`, removing its swap file.
    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }

        let tab = self.tabs.remove(index);
        if let Err(err) = self.store.remove_swap(tab.id) {
            log::error!("failed to remove swap file for tab {}: {err}", tab.id);
        }

        if self.tabs.is_empty() {
            let id = self.alloc_id();
            self.tabs.push(Tab::new_untitled(id));
            self.active = 0;
            self.persist_active_swap();
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }

        self.persist_session();
    }

    /// Show a native open-file dialog and load the chosen file into a tab.
    fn open_file(&mut self) {
        let Some(path) = rfd_pick_file() else {
            return;
        };

        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let id = self.alloc_id();
                let tab = Tab::from_file(id, path, text);
                if let Err(err) = self.store.save_swap(&tab) {
                    log::error!("failed to write swap for opened file: {err}");
                }
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                self.persist_session();
            }
            Err(err) => log::error!("failed to open {}: {err}", path.display()),
        }
    }

    /// Save the active tab. Prompts for a path when the buffer is untitled.
    fn save_active(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };

        let path = match tab.path.clone() {
            Some(path) => path,
            None => match rfd_save_file() {
                Some(path) => path,
                None => return,
            },
        };

        if let Err(err) = std::fs::write(&path, &tab.text) {
            log::error!("failed to save {}: {err}", path.display());
            return;
        }

        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.path = Some(path);
            tab.saved_text = tab.text.clone();
        }
        self.persist_session();
    }
}

impl eframe::App for FrextApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut action = MenuAction::None;

        // Keyboard shortcuts.
        ctx.input_mut(|i| {
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::N,
            )) {
                action = MenuAction::NewTab;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::O,
            )) {
                action = MenuAction::Open;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::S,
            )) {
                action = MenuAction::Save;
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::W,
            )) {
                action = MenuAction::CloseActive;
            }
        });

        egui::Panel::top("frext_tab_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("\u{002b}")
                    .on_hover_text("New tab (Ctrl+N)")
                    .clicked()
                {
                    action = MenuAction::NewTab;
                }
                if ui
                    .button("Open")
                    .on_hover_text("Open file (Ctrl+O)")
                    .clicked()
                {
                    action = MenuAction::Open;
                }
                if ui.button("Save").on_hover_text("Save (Ctrl+S)").clicked() {
                    action = MenuAction::Save;
                }
                ui.separator();

                for index in 0..self.tabs.len() {
                    let selected = index == self.active;
                    let title = self.tabs[index].title();
                    let label = if selected {
                        egui::RichText::new(title).color(crate::theme::accent())
                    } else {
                        egui::RichText::new(title)
                    };
                    if ui.selectable_label(selected, label).clicked() {
                        action = MenuAction::Select(index);
                    }
                    if ui
                        .small_button("\u{00d7}")
                        .on_hover_text("Close tab (Ctrl+W)")
                        .clicked()
                    {
                        action = MenuAction::Close(index);
                    }
                    ui.separator();
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(tab) = self.tabs.get_mut(self.active) {
                let response = ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::multiline(&mut tab.text)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );

                if response.changed() {
                    self.persist_active_swap();
                }
            }
        });

        match action {
            MenuAction::None => {}
            MenuAction::NewTab => self.new_tab(),
            MenuAction::Open => self.open_file(),
            MenuAction::Save => self.save_active(),
            MenuAction::CloseActive => self.close_tab(self.active),
            MenuAction::Close(index) => self.close_tab(index),
            MenuAction::Select(index) => {
                self.active = index;
                self.persist_session();
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Final flush: make sure swap files and the index are current.
        for tab in &self.tabs {
            if let Err(err) = self.store.save_swap(tab) {
                log::error!("failed to flush swap for tab {} on exit: {err}", tab.id);
            }
        }
        self.persist_session();
    }
}

/// A deferred action chosen from the menu/tab bar, applied after the borrow
/// of `self.tabs` ends.
enum MenuAction {
    None,
    NewTab,
    Open,
    Save,
    CloseActive,
    Close(usize),
    Select(usize),
}

/// Show a native "open file" picker. Returns `None` if cancelled or if no
/// picker backend is available.
fn rfd_pick_file() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().pick_file()
}

/// Show a native "save file" picker. Returns `None` if cancelled.
fn rfd_save_file() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().save_file()
}
