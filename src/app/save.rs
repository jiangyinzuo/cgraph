use std::path::Path;

use super::App;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SaveStatus {
    Editing,
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveState {
    pub input: String,
    pub status: SaveStatus,
}

impl App {
    pub fn open_save(&mut self) {
        self.pending_key = None;
        self.clear_canvas_notice();
        self.save = Some(SaveState {
            input: String::new(),
            status: SaveStatus::Editing,
        });
    }

    pub fn close_save(&mut self) {
        self.save = None;
    }

    pub fn push_save_char(&mut self, character: char) {
        let Some(save) = self.save.as_mut() else {
            return;
        };
        save.input.push(character);
        save.status = SaveStatus::Editing;
    }

    pub fn pop_save_char(&mut self) {
        let Some(save) = self.save.as_mut() else {
            return;
        };
        save.input.pop();
        save.status = SaveStatus::Editing;
    }

    pub fn fail_save(&mut self, error: impl Into<String>) {
        let error = error.into();
        if let Some(save) = self.save.as_mut() {
            save.status = SaveStatus::Error(error.clone());
        }
        self.set_canvas_error(format!("Save failed: {error}"));
    }

    pub fn complete_save(&mut self, path: &Path) {
        self.save = None;
        self.set_canvas_notice(format!("Saved graph to {}", path.display()));
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{super::App, SaveStatus};
    use crate::cli::Cli;

    #[test]
    fn edits_save_state_and_keeps_errors_until_the_next_edit() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        app.open_save();
        app.push_save_char('图');
        app.push_save_char('.');
        app.push_save_char('t');
        app.pop_save_char();
        assert_eq!(app.save.as_ref().unwrap().input, "图.");

        app.fail_save("target exists");
        assert_eq!(
            app.save.as_ref().unwrap().status,
            SaveStatus::Error("target exists".to_owned())
        );
        app.push_save_char('t');
        assert_eq!(app.save.as_ref().unwrap().status, SaveStatus::Editing);
        app.close_save();
        assert!(app.save.is_none());
    }
}
