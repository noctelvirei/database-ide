//! The workbench's part in "Files, restored tabs and history" (docs/spec.md): it restores
//! the tabs kept from the last session, keeps them again a second after typing stops and on
//! quit, opens `.sql` files, signs restored tabs in on their first run, adds each run to the
//! query history, and opens the history panel.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{AppContext as _, Context, Entity, Window};

use super::Workbench;
use crate::history::{self, Entry};
use crate::history_panel::HistoryPanel;
use crate::open_tabs::{self, SavedTab};
use crate::query_tab::file::{QueryTabEvent, TabFile};
use crate::query_tab::{QueryTab, QueryTarget};

/// How long after the last keystroke the open tabs are kept.
const SAVE_DELAY: Duration = Duration::from_secs(1);

impl Workbench {
    /// Opens the tabs kept from the last session, not connected until each one's first run,
    /// or one unconnected tab when none were kept.
    pub(super) fn restore_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kept = match self.tabs_file.as_deref().map(open_tabs::load) {
            Some(Ok(kept)) => Ok(kept),
            None => Ok(Vec::new()),
            // Saving would replace the tabs it couldn't read, so this session keeps none.
            Some(Err(error)) => Err(format!(
                "Couldn't read the tabs kept from last time ({error}); this session won't keep \
                 its tabs, so the file stays as it is."
            )),
        };
        if kept.is_err() {
            self.tabs_file = None;
        }
        for saved in kept.clone().unwrap_or_default() {
            self.open_saved(saved, window, cx);
        }
        match self.tabs.is_empty() {
            true => self.open_tab(None, window, cx),
            false => self.show_tab(0, window, cx),
        }
        if let Err(problem) = kept {
            self.note(problem, cx);
        }
    }

    /// Opens `saved` in a new tab on its saved connection, not connected until its first
    /// run. A tab whose connection was deleted opens unconnected.
    fn open_saved(&mut self, saved: SavedTab, window: &mut Window, cx: &mut Context<Self>) {
        let target = saved.connection_id.as_deref().and_then(|id| {
            self.explorer
                .read(cx)
                .restored_target(id, saved.database.clone())
        });
        let file = saved.file.map(|path| TabFile::reopen(path, saved.encoding));
        let (title, runtime) = (self.next_title(), self.runtime.clone());
        let tab =
            cx.new(|cx| QueryTab::restored(title, target, runtime, &saved.text, file, window, cx));
        self.add_tab(tab, window, cx);
    }

    /// What a tab asks of the workbench.
    pub(super) fn on_tab_event(
        &mut self,
        tab: &Entity<QueryTab>,
        event: &QueryTabEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            QueryTabEvent::SignIn(id) => {
                // The explorer's sign-in is modal, so a tab still waiting from before had its
                // sign-in cancelled; it must not run when this one signs in.
                for earlier in std::mem::take(&mut self.waking) {
                    if let Some(earlier) = earlier.upgrade().filter(|e| e != tab) {
                        earlier.update(cx, |earlier, _| earlier.stop_waiting());
                    }
                }
                self.waking.push(tab.downgrade());
                let found = self
                    .explorer
                    .update(cx, |explorer, cx| explorer.resume(id, window, cx));
                if !found {
                    tab.update(cx, |tab, cx| {
                        tab.note("This tab's saved connection was deleted.", cx);
                    });
                }
            }
            QueryTabEvent::Ran(entry) => self.add_to_history(entry.clone(), cx),
        }
    }

    /// The explorer signed in for a restored tab: the one waiting on that connection
    /// connects and runs.
    pub(super) fn resume_waking(
        &mut self,
        target: &QueryTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let waiting = std::mem::take(&mut self.waking);
        for weak in waiting {
            let Some(tab) = weak.upgrade() else { continue };
            let same = tab
                .read(cx)
                .target()
                .is_some_and(|t| t.saved.id == target.saved.id);
            match same {
                true => tab.update(cx, |tab, cx| tab.resume(target.clone(), window, cx)),
                false => self.waking.push(weak),
            }
        }
    }

    /// Appends `entry` to the history file off the UI thread.
    fn add_to_history(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let Some(path) = self.history_file.clone() else {
            return;
        };
        let appending = cx.background_spawn(async move { history::append(&path, &entry) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = appending.await {
                let _ = this.update(cx, |this, cx| {
                    this.note(
                        format!("Couldn't add the run to the query history: {error}"),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    /// Keeps the open tabs `SAVE_DELAY` after the last call.
    pub(super) fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_generation += 1;
        let generation = self.save_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.save_generation == generation {
                    this.save_tabs(cx);
                }
            });
        })
        .detach();
    }

    /// Keeps the open tabs now, saying so in the strip when the file can't be written.
    pub(super) fn save_tabs(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.tabs_file.clone() else {
            return;
        };
        let tabs: Vec<SavedTab> = self.tabs.iter().map(|t| t.read(cx).saved(cx)).collect();
        if let Err(error) = open_tabs::save(&path, &tabs) {
            self.note(
                format!("Couldn't keep the open tabs in {}: {error}", path.display()),
                cx,
            );
        }
    }

    /// Ctrl+O on a `.sql` file: reads it off the UI thread, then opens it in a new tab on the
    /// active tab's connection.
    pub(super) fn open_sql_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reading = {
            let path = path.clone();
            cx.background_spawn(async move { crate::sql_file::read(&path) })
        };
        self.opening = true;
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.opening = false;
                match read {
                    Ok((text, encoding)) => {
                        this.open_beside_active(window, cx);
                        let tab = this.active_tab().clone();
                        tab.update(cx, |tab, cx| {
                            tab.load_file(path, text, encoding, window, cx)
                        });
                        this.schedule_save(cx);
                    }
                    Err(error) => {
                        this.note(format!("Couldn't open {}: {error}", path.display()), cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Opens an empty tab on the active tab's connection and database. Beside a restored
    /// tab that hasn't signed in yet, the new tab also waits for its first run, since the
    /// active tab holds no password to connect with.
    pub(super) fn open_beside_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active_tab().read(cx);
        if !active.is_asleep() {
            let target = active.target().cloned();
            self.open_tab(target, window, cx);
            return;
        }
        let kept = active.saved(cx);
        self.open_saved(
            SavedTab {
                connection_id: kept.connection_id,
                database: kept.database,
                ..SavedTab::default()
            },
            window,
            cx,
        );
        self.schedule_save(cx);
    }

    /// Ctrl+S outside a grid or plan: saves the active tab to its `.sql` file.
    pub(super) fn save_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.active_tab().clone();
        tab.update(cx, |tab, cx| tab.save_file(window, cx));
        self.schedule_save(cx);
    }

    /// Ctrl+Shift+H: loads the history off the UI thread and opens the panel over it.
    pub(super) fn show_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.history_file.clone();
        let loading = cx.background_spawn(async move {
            path.map(|path| history::load(&path, history::now_ms()))
                .unwrap_or_default()
        });
        self.opening = true;
        cx.spawn_in(window, async move |this, cx| {
            let entries = loading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.opening = false;
                let workbench = cx.entity().downgrade();
                let closing = workbench.clone();
                let reopen = move |entry: Entry, window: &mut Window, cx: &mut gpui::App| {
                    let _ = workbench.update(cx, |this, cx| this.reopen(entry, window, cx));
                };
                let closed = move |_: &mut Window, cx: &mut gpui::App| {
                    let _ = closing.update(cx, |this, cx| {
                        this.history = None;
                        cx.notify();
                    });
                };
                this.history = Some(HistoryPanel::open(entries, reopen, closed, window, cx));
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens a history entry's text in a new tab on its connection and database.
    fn reopen(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        self.history = None;
        self.open_saved(
            SavedTab {
                text: entry.text,
                connection_id: entry.connection_id,
                database: entry.database,
                ..SavedTab::default()
            },
            window,
            cx,
        );
        self.schedule_save(cx);
    }
}
