//! Scroll overflow fades using the wgpu content alpha-mask API.

use gpui::*;
use gpui_base::ScrollbarHandle;

/// Fade only the overflowing edges of a viewport. The handle is read during
/// paint, after layout and scroll updates, so wheel, keyboard and thumb drags
/// all use the current position. Keep the scrollbar outside this mask.
pub fn scroll_viewport<H: ScrollbarHandle + Clone, C: IntoElement>(
    child: C,
    handle: &H,
) -> impl IntoElement + use<H, C> {
    ScrollMask {
        child: child.into_any_element(),
        handle: handle.clone(),
    }
}

struct ScrollMask<H> {
    child: AnyElement,
    handle: H,
}

impl<H: ScrollbarHandle> IntoElement for ScrollMask<H> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl<H: ScrollbarHandle> Element for ScrollMask<H> {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let viewport = self.handle.viewport_bounds().size.height;
        let max = (self.handle.content_size().height - viewport).max(px(0.));
        let offset = -self.handle.offset().y;
        let above = offset > px(1.);
        let below = offset < max - px(1.);
        if (!above && !below) || bounds.size.height <= px(0.) {
            self.child.paint(window, cx);
            return;
        }
        let edge = (px(20.) / bounds.size.height).min(0.25);
        let mask = linear_gradient(
            180.,
            linear_color_stop(rgba(0xffffffff), 0.),
            linear_color_stop(rgba(0xffffffff), 1.),
        )
        .with_stops([
            linear_color_stop(
                if above {
                    rgba(0xffffff00)
                } else {
                    rgba(0xffffffff)
                },
                0.,
            ),
            linear_color_stop(rgba(0xffffffff), edge),
            linear_color_stop(rgba(0xffffffff), 1. - edge),
            linear_color_stop(
                if below {
                    rgba(0xffffff00)
                } else {
                    rgba(0xffffffff)
                },
                1.,
            ),
        ]);
        window.with_filter_layer(
            bounds,
            Corners::default(),
            &[Filter::Mask(mask)],
            |window| {
                self.child.paint(window, cx);
            },
        );
    }
}
