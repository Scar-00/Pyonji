pub mod button;

use anyhow::Result;
use button::Button;

pub struct Overlay {}

impl Overlay {
    pub fn new() -> Self {
        Self {}
    }

    pub fn render(&mut self) -> Result<()> {
        Ok(())
    }
}
