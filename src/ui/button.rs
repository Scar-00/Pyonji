use winit::event::MouseButton;

pub struct Button {
    label: String,
    on_click: Option<Box<dyn Fn()>>,
}

impl Button {
    pub fn new(label: impl ToString) -> Self {
        Self {
            label: label.to_string(),
            on_click: None,
        }
    }

    pub fn on_click(mut self, f: impl 'static + Fn()) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }

    pub fn handle_input(&self, event: MouseButton) {}

    pub fn render(&mut self) {}
}
