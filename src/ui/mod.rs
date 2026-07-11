pub mod context;
pub mod div;
pub mod entity;
pub mod render;
pub mod renderer;
pub mod types;

use crate::ui::{
    entity::{Entity, EntityId},
    render::{AnyElement, Element},
    renderer::UiRenderer,
    types::{Pixel, Point, Size, Style},
};
use anyhow::Result;
use context::Context;
use entity::EntityMap;
pub use render::{IntoElement, Render};
use std::collections::HashSet;
use std::sync::Arc;
use taffy::{prelude, NodeId, TaffyTree};
use winit::{dpi::PhysicalSize, window::Window};

pub type ElementId = NodeId;

pub struct UiLayer {
    window: Arc<Window>,
    entity_map: EntityMap,
    updated_entities: HashSet<EntityId>,
    pub layout_tree: TaffyTree<()>,
    root: Option<AnyElement>,

    local: LocalExecuter<'static>,
    threaded: ThreadedExecuter<'static>,
}

impl UiLayer {
    pub fn ndc_pos(&self, coords: Point<Pixel>) -> Point<f32> {
        let size = self.window.inner_size();
        let Point { x, y } = coords;
        Point {
            x: (*x as f32 / size.width as f32) * 2.0 - 1.0,
            y: 1.0 - (*y as f32 / size.height as f32) * 2.0,
        }
    }

    pub fn ndc_size(&self, coords: Size<Pixel>) -> Size<f32> {
        let size = self.window.inner_size();
        let Size { width, height } = coords;
        Size {
            width: (*width as f32 / size.width as f32) * 2.0,
            height: (*height as f32 / size.height as f32) * 2.0,
        }
    }

    pub fn new_init(window: Arc<Window>) -> Result<Self> {
        Ok(Self {
            window,
            entity_map: EntityMap::new(),
            updated_entities: HashSet::new(),
            layout_tree: TaffyTree::new(),
            root: None,

            local: LocalExecuter::new(),
            threaded: ThreadedExecuter::new(),
        })
    }

    #[track_caller]
    pub fn open<T: 'static + Render>(
        &mut self,
        builder: impl FnOnce(&mut Context<T>) -> T,
    ) -> Result<()> {
        let entity = self.new(builder);
        let mut lease = self.entity_map.lease::<T>(entity.id);
        let root = lease.render(self).into_element().into_any();
        self.entity_map.end_lease(lease);
        self.root = Some(root);
        Ok(())
    }

    pub fn layout_element(&mut self, style: &Style, children: &[ElementId]) -> ElementId {
        let style = style.into();
        if children.is_empty() {
            self.layout_tree.new_leaf(style).expect("taffy error")
        } else {
            self.layout_tree
                .new_with_children(style, children)
                .expect("taffy error")
        }
    }

    pub fn new<T: 'static>(&mut self, f: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        let slot = self.entity_map.reserve();
        let value = f(&mut Context::new(slot.handle.clone(), self));
        self.entity_map.insert(slot, value)
    }

    #[track_caller]
    pub fn update_entity<T: 'static, R>(
        &mut self,
        entity: &Entity<T>,
        update: impl Fn(&mut T, &mut Context<T>) -> R,
    ) -> R {
        let id = entity.id;
        let mut lease = self.entity_map.lease(id);
        let res = update(&mut lease, &mut Context::new(entity.clone(), self));
        self.entity_map.end_lease(lease);
        res
    }

    pub fn notify(&mut self, entity_id: EntityId) {
        self.updated_entities.insert(entity_id);
    }

    pub fn layout(&mut self, size: PhysicalSize<u32>) {
        self.update_root(|this, root| {
            let id = root.layout(this);
            _ = this.layout_tree.compute_layout(
                id,
                taffy::Size {
                    width: taffy::AvailableSpace::Definite(size.width as f32),
                    height: taffy::AvailableSpace::Definite(size.height as f32),
                },
            );
        });
    }

    pub fn prepaint(&mut self) {
        self.update_root(|this, root| {
            root.prepaint(this);
        });
    }

    pub fn paint(&mut self, renderer: &mut UiRenderer) -> Result<()> {
        self.update_root(|this, root| {
            root.paint(this, renderer);
        });
        Ok(())
    }

    fn update_root(&mut self, f: impl FnOnce(&mut Self, &mut AnyElement)) {
        let Some(mut root) = self.root.take() else {
            return;
        };
        f(self, &mut root);
        self.root = Some(root);
    }
}

impl AppContext for UiLayer {
    fn new<T: 'static>(&mut self, f: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        self.new(f)
    }

    fn update_entity<T: 'static, R>(
        &mut self,
        entity: &Entity<T>,
        update: impl Fn(&mut T, &mut Context<T>) -> R,
    ) -> R {
        self.update_entity(entity, update)
    }
}

pub trait AppContext {
    fn new<T: 'static>(&mut self, f: impl FnOnce(&mut Context<T>) -> T) -> Entity<T>;
    fn update_entity<T: 'static, R>(
        &mut self,
        entity: &Entity<T>,
        update: impl Fn(&mut T, &mut Context<T>) -> R,
    ) -> R;
}

#[derive()]
pub struct LocalExecuter<'exec>(smol::LocalExecutor<'exec>);

impl<'exec> LocalExecuter<'exec> {
    fn new() -> Self {
        Self(smol::LocalExecutor::new())
    }
}

pub struct ThreadedExecuter<'exec>(smol::Executor<'exec>);

impl<'exec> ThreadedExecuter<'exec> {
    fn new() -> Self {
        Self(smol::Executor::new())
    }
}
