use std::any::Any;

use crate::ui::types::*;
use crate::ui::{renderer::UiRenderer, ElementId, UiLayer};

pub trait Render {
    fn render(&mut self, cx: &mut UiLayer) -> impl IntoElement;
}

pub trait Element: 'static + IntoElement {
    fn layout(&mut self, cx: &mut UiLayer) -> ElementId;
    fn prepaint(&mut self, cx: &mut UiLayer, id: ElementId, bounds: Rect);
    fn paint(&mut self, cx: &mut UiLayer, renderer: &mut UiRenderer, id: ElementId, bounds: Rect);

    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}

pub trait IntoElement: Sized {
    type Element: Element;

    fn into_element(self) -> Self::Element;
}

pub struct AnyElement {
    pub id: Option<ElementId>,
    element: Box<dyn Any>,
    layout: Box<dyn Fn(&mut dyn Any, &mut UiLayer) -> ElementId>,
    prepaint: Box<dyn Fn(&mut dyn Any, &mut UiLayer, ElementId, Rect)>,
    paint: Box<dyn Fn(&mut dyn Any, &mut UiLayer, &mut UiRenderer, ElementId, Rect)>,
}

impl AnyElement {
    fn new<E: Element>(e: E) -> Self {
        Self {
            id: None,
            element: Box::new(e),
            layout: Box::new(|e, layer| {
                let e = e.downcast_mut::<E>().unwrap();
                E::layout(e, layer)
            }),
            prepaint: Box::new(|e, layer, id, bounds| {
                let e = e.downcast_mut::<E>().unwrap();
                E::prepaint(e, layer, id, bounds)
            }),
            paint: Box::new(|e, layer, renderer, id, bounds| {
                let e = e.downcast_mut::<E>().unwrap();
                E::paint(e, layer, renderer, id, bounds)
            }),
        }
    }
}

impl AnyElement {
    fn layout(&mut self, cx: &mut UiLayer) -> ElementId {
        let element = &mut self.element;
        let id = (self.layout)(element.as_mut(), cx);
        self.id = Some(id);
        id
    }

    pub fn prepaint(&mut self, cx: &mut UiLayer) {
        let id = self.id.unwrap();
        let element = &mut self.element;
        let layout = cx.layout_tree.layout(id).unwrap();
        (self.prepaint)(element.as_mut(), cx, id, layout.into())
    }

    pub fn paint(&mut self, cx: &mut UiLayer, renderer: &mut UiRenderer) {
        let id = self.id.unwrap();
        let element = &mut self.element;
        let layout = cx.layout_tree.layout(id).unwrap();
        (self.paint)(element.as_mut(), cx, renderer, id, layout.into())
    }
}

impl Element for AnyElement {
    fn layout(&mut self, cx: &mut UiLayer) -> ElementId {
        self.layout(cx)
    }

    fn prepaint(&mut self, cx: &mut UiLayer, _: ElementId, _: Rect) {
        self.prepaint(cx);
    }

    fn paint(&mut self, cx: &mut UiLayer, renderer: &mut UiRenderer, _: ElementId, _: Rect) {
        self.paint(cx, renderer);
    }
}

impl IntoElement for AnyElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl IntoElement for () {
    type Element = ();

    fn into_element(self) -> Self::Element {
        ()
    }
}

impl Element for () {
    fn layout(&mut self, cx: &mut UiLayer) -> ElementId {
        cx.layout_element(&Style::default(), &[])
    }

    fn prepaint(&mut self, _: &mut UiLayer, _: ElementId, _: Rect) {}
    fn paint(&mut self, _: &mut UiLayer, _: &mut UiRenderer, _: ElementId, _: Rect) {}
}
