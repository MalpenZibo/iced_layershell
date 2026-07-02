//! Compositor-side background blur (`ext-background-effect-v1`), driven from the
//! widget tree: [`blur_container`] is a [`container`] that also publishes a blur
//! region derived from its own bounds and the corner radius of its style. The
//! backend gathers all blur widgets each frame via [`CollectBlurRegions`].

use std::cell::Cell;

use iced_core::border::{Border, Radius};
use iced_core::widget::tree::{self, Tree};
use iced_core::widget::{Id, Operation, Widget};
use iced_core::{
    Clipboard, Element, Event, Layout, Length, Padding, Rectangle, Shell, Size, Vector, alignment,
    layout, mouse, overlay, renderer,
};
use iced_widget::container;

use crate::blur_region::rounded_rect_to_blur_rects;
use crate::task_impl::BlurRect;

/// Marker read by [`CollectBlurRegions`]. A [`Cell`] because the radius is
/// resolved from the theme in `draw` (`&Tree` only) and read back in `operate`.
#[derive(Debug, Default)]
struct BlurTag {
    radius: Cell<Radius>,
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
    BlurContainer {
        content: content.into(),
        style: Box::new(|_| container::Style::default()),
        width: Length::Shrink,
        height: Length::Shrink,
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
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<BlurTag>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(BlurTag::default())
    }

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
        operation.custom(None, layout.bounds(), tree.state.downcast_mut::<BlurTag>());
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
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

        // Hand the resolved radius to `operate` for the blur region.
        tree.state
            .downcast_ref::<BlurTag>()
            .radius
            .set(style.border.radius);

        if let Some(clipped_viewport) = bounds.intersection(viewport) {
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

/// Gathers the bounds and corner radius of every blur widget in a surface tree.
pub(crate) struct CollectBlurRegions {
    regions: Vec<(Rectangle, Radius)>,
}

impl CollectBlurRegions {
    pub(crate) fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    pub(crate) fn into_blur_rects(self) -> Vec<BlurRect> {
        self.regions
            .into_iter()
            .flat_map(|(bounds, radius)| rounded_rect_to_blur_rects(bounds, radius))
            .collect()
    }
}

impl<T> Operation<T> for CollectBlurRegions {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn custom(&mut self, _id: Option<&Id>, bounds: Rectangle, state: &mut dyn std::any::Any) {
        if let Some(tag) = state.downcast_ref::<BlurTag>() {
            self.regions.push((bounds, tag.radius.get()));
        }
    }
}
