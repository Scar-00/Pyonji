use crate::{
    renderer::Color,
    ui::{
        render::{AnyElement, Element},
        renderer::UiRenderer,
        types::*,
        ElementId, IntoElement, UiLayer,
    },
};

pub fn div() -> Div {
    Div::default()
}

#[derive(Default)]
pub struct Div {
    children: Vec<AnyElement>,
    style: Style,
}

impl Div {
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_element().into_any());
        self
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
}

impl Element for Div {
    fn layout(&mut self, cx: &mut UiLayer) -> ElementId {
        let children = self
            .children
            .iter_mut()
            .map(|child| child.layout(cx))
            .collect::<Vec<_>>();
        cx.layout_element(&self.style, &children)
    }

    fn prepaint(&mut self, cx: &mut UiLayer, _: ElementId, _bounds: Rect) {
        println!("{_bounds:#?}");
        self.children
            .iter_mut()
            .for_each(|child| AnyElement::prepaint(child, cx));
    }

    fn paint(&mut self, cx: &mut UiLayer, renderer: &mut UiRenderer, _: ElementId, bounds: Rect) {
        let Point { x, y } = cx.ndc_pos(bounds.origin);
        let Size { width, height } = cx.ndc_size(bounds.size);
        if let Some(color) = &self.style.color {
            renderer.add_rect(x, y, width, height, color.inner());
        }
        self.children
            .iter_mut()
            .for_each(|child| child.paint(cx, renderer));
    }
}

impl IntoElement for Div {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
