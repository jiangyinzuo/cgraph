#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HelpState {
    pub scroll: usize,
}

use super::App;

impl App {
    pub fn open_help(&mut self) {
        self.pending_key = None;
        self.help = Some(HelpState::default());
    }

    pub fn close_help(&mut self) {
        self.help = None;
    }
}
