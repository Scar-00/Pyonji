use crate::ui::{AppContext, Entity, UiLayer};

pub struct Context<'a, T: 'static> {
    entity: Entity<T>,
    app: &'a mut UiLayer,
}

impl<'a, T: 'static> Context<'a, T> {
    pub fn new(entity: Entity<T>, app: &'a mut UiLayer) -> Self {
        Self { entity, app }
    }

    pub fn notify(&mut self) {
        self.app.notify(self.entity.id);
    }
}

impl<'a, B: 'static> AppContext for Context<'a, B> {
    fn new<T: 'static>(&mut self, f: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        self.app.new(f)
    }

    fn update_entity<T: 'static, R>(
        &mut self,
        entity: &Entity<T>,
        update: impl Fn(&mut T, &mut Context<T>) -> R,
    ) -> R {
        self.app.update_entity(entity, update)
    }
}
