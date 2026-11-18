//! Windows' file dialogs, for opening and saving files. Under `--ui-script` no dialog opens:
//! each prompt takes the next path the script queued with a `path` input, and a prompt with
//! none queued reads as cancelled, so a script never stops at a window it can't answer.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use futures::FutureExt as _;
use futures::future::{self, BoxFuture};
use gpui::{App, Global, PathPromptOptions};

/// The paths a UI script has queued for the next prompts, oldest first. Its presence marks
/// the app as playing a script.
#[derive(Default)]
pub struct ScriptedPaths(VecDeque<PathBuf>);

impl Global for ScriptedPaths {}

impl ScriptedPaths {
    /// Queues `path` as the answer to the next prompt.
    pub fn push(&mut self, path: PathBuf) {
        self.0.push_back(path);
    }
}

/// Asks for one existing file to open. `None` when the user cancels.
pub fn open_path(cx: &mut App) -> BoxFuture<'static, Option<PathBuf>> {
    if let Some(answer) = scripted(cx) {
        return future::ready(answer).boxed();
    }
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    });
    async move {
        let paths = chosen.await.ok()?.ok()??;
        paths.into_iter().next()
    }
    .boxed()
}

/// Asks where to save a new file, suggesting `name`. `None` when the user cancels.
pub fn save_path(cx: &mut App, name: &str) -> BoxFuture<'static, Option<PathBuf>> {
    if let Some(answer) = scripted(cx) {
        return future::ready(answer).boxed();
    }
    let directory = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let chosen = cx.prompt_for_new_path(&directory, Some(name));
    async move { chosen.await.ok()?.ok()? }.boxed()
}

/// Under a script, the queued answer (`Some(None)` for none queued); otherwise `None`.
fn scripted(cx: &mut App) -> Option<Option<PathBuf>> {
    cx.has_global::<ScriptedPaths>()
        .then(|| cx.global_mut::<ScriptedPaths>().0.pop_front())
}
