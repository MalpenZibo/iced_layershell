//! Compositor-side background blur (`ext-background-effect-v1`), driven from the
//! widget tree: [`blur_container`] is a [`container`] that also publishes a blur
//! region derived from its own bounds and the corner radius of its style.
//!
//! Regions are recorded during `draw` rather than by a second [`Operation`] pass
//! over the tree: `draw` already resolves the style (and with it the radius) and
//! already knows what a parent clipped away, so the backend gets the regions for
//! free instead of walking every widget again on each frame.

use std::cell::{Cell, RefCell};

use iced_core::border::{Border, Radius};
use iced_core::widget::Tree;
use iced_core::widget::{Operation, Widget};
use iced_core::{
    Clipboard, Element, Event, Layout, Length, Padding, Rectangle, Shell, Size, Vector, alignment,
    layout, mouse, overlay, renderer,
};
use iced_widget::container;

use crate::blur_region::rounded_rect_to_blur_rects;
use crate::task_impl::BlurRect;

thread_local! {
    /// Regions recorded by [`BlurContainer::draw`] since the last [`begin_frame`].
    static REGIONS: RefCell<Vec<(Rectangle, Radius)>> = const { RefCell::new(Vec::new()) };
    static COLLECTING: Cell<bool> = const { Cell::new(false) };
}

/// Start recording regions for one surface's draw. `enabled` is false when the
/// compositor can't blur, which makes recording a single [`Cell`] read.
pub(crate) fn begin_frame(enabled: bool) {
    COLLECTING.set(enabled);
    REGIONS.with_borrow_mut(Vec::clear);
}

/// Take what the last draw recorded, converting iced's logical coordinates into
/// the surface-local pixels `wl_region` expects.
///
/// The viewport scale is `monitor_scale * app_scale` while the surface is sized
/// in `monitor_scale`-relative pixels, so only `app_scale` has to be undone.
/// Scaling before tessellation keeps the pixel rounding in the target space.
pub(crate) fn take_rects(app_scale: f32) -> Vec<BlurRect> {
    REGIONS.with_borrow_mut(|regions| {
        regions
            .drain(..)
            .flat_map(|(bounds, radius)| {
                rounded_rect_to_blur_rects(bounds * app_scale, radius * app_scale)
            })
            .collect()
    })
}

fn record(bounds: Rectangle, radius: Radius) {
    if COLLECTING.get() {
        REGIONS.with_borrow_mut(|regions| regions.push((bounds, radius)));
    }
}

/// A [`container`] that also blurs the compositor background behind itself,
/// shaped by the corner radius of its style. Build one with [`blur_container`].
#[allow(missing_debug_implementations)]
pub struct BlurContainer<'a, Message, Theme = iced_core::Theme, Renderer = iced_renderer::Renderer>
where
    Renderer: iced_core::Renderer,
{
    content: Element<'a, Message, Theme, Renderer>,
    style: Box<dyn Fn(&Theme) -> container::Style + 'a>,
    width: Length,
    height: Length,
    max_width: f32,
    max_height: f32,
    padding: Padding,
    horizontal_alignment: alignment::Horizontal,
    vertical_alignment: alignment::Vertical,
    clip: bool,
}

/// Create a [`container`] that blurs the compositor background behind itself,
/// following its bounds and the corner radius of its style. No-ops when the
/// compositor lacks `ext-background-effect-v1`.
pub fn blur_container<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> BlurContainer<'a, Message, Theme, Renderer>
where
    Renderer: iced_core::Renderer,
{
    let content = content.into();
    // Adopt the content's fluidity, exactly like `Container::new`. Hardcoding
    // `Shrink` collapses a `Fill` child to zero size, which silently makes the
    // whole widget disappear.
    let size = content.as_widget().size_hint();

    BlurContainer {
        content,
        style: Box::new(|_| container::Style::default()),
        width: size.width.fluid(),
        height: size.height.fluid(),
        max_width: f32::INFINITY,
        max_height: f32::INFINITY,
        padding: Padding::ZERO,
        horizontal_alignment: alignment::Horizontal::Left,
        vertical_alignment: alignment::Vertical::Top,
        clip: false,
    }
}

/// Mark `content` for background blur with an explicit corner `radius`, drawing
/// no background of its own. Prefer [`blur_container`] when the surface already
/// draws a rounded translucent background (single source of truth for the radius).
pub fn blur<'a, Message, Theme, Renderer>(
    radius: impl Into<Radius>,
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> BlurContainer<'a, Message, Theme, Renderer>
where
    Renderer: iced_core::Renderer,
{
    let radius = radius.into();
    blur_container(content).style(move |_| container::Style {
        border: Border {
            radius,
            ..Border::default()
        },
        ..container::Style::default()
    })
}

impl<'a, Message, Theme, Renderer> BlurContainer<'a, Message, Theme, Renderer>
where
    Renderer: iced_core::Renderer,
{
    /// Set the style. Its `border.radius` also shapes the blur region.
    #[must_use]
    pub fn style(mut self, style: impl Fn(&Theme) -> container::Style + 'a) -> Self {
        self.style = Box::new(style);
        self
    }

    /// Set the width.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set the height.
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Set the maximum width.
    #[must_use]
    pub fn max_width(mut self, max_width: impl Into<iced_core::Pixels>) -> Self {
        self.max_width = max_width.into().0;
        self
    }

    /// Set the maximum height.
    #[must_use]
    pub fn max_height(mut self, max_height: impl Into<iced_core::Pixels>) -> Self {
        self.max_height = max_height.into().0;
        self
    }

    /// Set the padding.
    #[must_use]
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Align the contents horizontally.
    #[must_use]
    pub fn align_x(mut self, alignment: impl Into<alignment::Horizontal>) -> Self {
        self.horizontal_alignment = alignment.into();
        self
    }

    /// Align the contents vertically.
    #[must_use]
    pub fn align_y(mut self, alignment: impl Into<alignment::Vertical>) -> Self {
        self.vertical_alignment = alignment.into();
        self
    }

    /// Center the contents on both axes, filling the given amount of space.
    #[must_use]
    pub fn center(self, length: impl Into<Length>) -> Self {
        let length = length.into();
        self.width(length)
            .height(length)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center)
    }

    /// Set whether the contents should be clipped to the bounds.
    #[must_use]
    pub fn clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for BlurContainer<'_, Message, Theme, Renderer>
where
    Renderer: iced_core::Renderer,
{
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        container::layout(
            limits,
            self.width,
            self.height,
            self.max_width,
            self.max_height,
            self.padding,
            self.horizontal_alignment,
            self.vertical_alignment,
            |limits| {
                self.content
                    .as_widget_mut()
                    .layout(&mut tree.children[0], renderer, limits)
            },
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                layout.children().next().unwrap(),
                renderer,
                operation,
            );
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout.children().next().unwrap(),
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().unwrap(),
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        renderer_style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let style = (self.style)(theme);

        if let Some(clipped_viewport) = bounds.intersection(viewport) {
            // Blur only what is actually on screen: a widget scrolled out of a
            // clipping parent must not leave blur behind where it isn't drawn.
            record(clipped_viewport, style.border.radius);

            container::draw_background(renderer, &style, bounds);

            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                &renderer::Style {
                    text_color: style.text_color.unwrap_or(renderer_style.text_color),
                },
                layout.children().next().unwrap(),
                cursor,
                if self.clip {
                    &clipped_viewport
                } else {
                    viewport
                },
            );
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<BlurContainer<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced_core::Renderer + 'a,
{
    fn from(blur: BlurContainer<'a, Message, Theme, Renderer>) -> Self {
        Element::new(blur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rectangle {
        Rectangle {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn take_rects_converts_logical_to_surface_local() {
        begin_frame(true);
        record(rect(10.0, 4.0, 100.0, 20.0), Radius::from(0.0));
        assert_eq!(
            take_rects(1.5),
            vec![BlurRect {
                x: 15,
                y: 6,
                width: 150,
                height: 30
            }]
        );
    }

    #[test]
    fn take_rects_is_identity_at_unit_scale() {
        begin_frame(true);
        record(rect(10.0, 4.0, 100.0, 20.0), Radius::from(0.0));
        assert_eq!(
            take_rects(1.0),
            vec![BlurRect {
                x: 10,
                y: 4,
                width: 100,
                height: 20
            }]
        );
    }

    #[test]
    fn fractional_scale_keeps_rounded_slabs_gap_free() {
        // A downscaled corner produces sub-pixel slabs; they must still tile the
        // band without gaps once rounded to whole pixels.
        begin_frame(true);
        record(rect(0.0, 0.0, 100.0, 40.0), Radius::from(8.0));
        let rects = take_rects(0.5);
        assert!(!rects.is_empty());
        for r in &rects {
            assert!(r.width > 0 && r.height > 0, "degenerate rect: {r:?}");
            assert!(r.x >= 0 && r.y >= 0 && r.x + r.width <= 50 && r.y + r.height <= 20);
        }
        let covers = |px: i32, py: i32| {
            rects
                .iter()
                .any(|r| px >= r.x && px < r.x + r.width && py >= r.y && py < r.y + r.height)
        };
        for y in 0..20 {
            assert!(covers(25, y), "column gap at y={y}");
        }
    }

    #[test]
    fn nothing_is_recorded_while_disabled() {
        begin_frame(false);
        record(rect(0.0, 0.0, 10.0, 10.0), Radius::from(0.0));
        assert!(take_rects(1.0).is_empty());
    }

    #[test]
    fn begin_frame_discards_the_previous_frame() {
        begin_frame(true);
        record(rect(0.0, 0.0, 10.0, 10.0), Radius::from(0.0));
        begin_frame(true);
        assert!(take_rects(1.0).is_empty());
    }
}
