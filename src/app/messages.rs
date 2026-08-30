use super::App;

impl App {
    pub fn set_canvas_notice(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.record_message(message.clone());
        self.canvas_notice = Some(message);
        self.canvas_notice_is_error = false;
    }

    pub fn set_canvas_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.record_message(message.clone());
        self.canvas_notice = Some(message);
        self.canvas_notice_is_error = true;
    }

    pub fn clear_canvas_notice(&mut self) {
        self.canvas_notice = None;
        self.canvas_notice_is_error = false;
    }

    pub fn canvas_notice_is_error(&self) -> bool {
        self.canvas_notice_is_error
    }

    fn record_message(&mut self, message: String) {
        if self.message_history.last() != Some(&message) {
            self.message_history.push(message);
        }
    }
}
