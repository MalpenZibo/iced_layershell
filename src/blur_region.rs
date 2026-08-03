//! Tessellation of rounded rectangles into `BlurRect` unions, since the
//! `ext-background-effect-v1` `wl_region` has no notion of curves.

use iced_core::{Rectangle, border::Radius};

use crate::task_impl::BlurRect;

/// Approximate a per-corner rounded rectangle as a union of axis-aligned
/// [`BlurRect`]s that **inscribe** the shape. A zero radius or degenerate
/// rectangle yields the bounding box.
///
/// `bounds` and `radius` must already be in surface-local pixels, so slab count
/// and edge rounding match the pixels the compositor blurs.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)] // slab counts/offsets are small, positive pixel quantities
pub(crate) fn rounded_rect_to_blur_rects(bounds: Rectangle, radius: Radius) -> Vec<BlurRect> {
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }

    let w = bounds.width;
    let h = bounds.height;

    // Clamp so two corners sharing an edge can never overlap.
    let max_r = w.min(h) / 2.0;
    let tl = radius.top_left.clamp(0.0, max_r);
    let tr = radius.top_right.clamp(0.0, max_r);
    let br = radius.bottom_right.clamp(0.0, max_r);
    let bl = radius.bottom_left.clamp(0.0, max_r);

    if tl.max(tr).max(br).max(bl) <= 0.5 {
        return Vec::from_iter(to_blur_rect(bounds));
    }

    let left_inset = |y: f32| corner_inset(y, tl, h - bl, bl);
    let right_inset = |y: f32| corner_inset(y, tr, h - br, br);

    let top_band = tl.max(tr);
    let bottom_band = bl.max(br);

    let mut out = Vec::new();

    if h - bottom_band > top_band {
        out.extend(to_blur_rect(Rectangle {
            x: bounds.x,
            y: bounds.y + top_band,
            width: w,
            height: h - bottom_band - top_band,
        }));
    }

    let mut emit_band = |y_start: f32, y_end: f32| {
        if y_end <= y_start {
            return;
        }
        let band_h = y_end - y_start;
        let steps = (band_h.ceil() as u32).clamp(4, 32);
        let slab_h = band_h / steps as f32;
        for i in 0..steps {
            let y0 = y_start + i as f32 * slab_h;
            let y1 = y0 + slab_h;
            // Widest inset within the slab keeps it inside the curve either way.
            let li = left_inset(y0).max(left_inset(y1));
            let ri = right_inset(y0).max(right_inset(y1));
            let slab_w = w - li - ri;
            if slab_w <= 0.0 {
                continue;
            }
            out.extend(to_blur_rect(Rectangle {
                x: bounds.x + li,
                y: bounds.y + y0,
                width: slab_w,
                height: slab_h,
            }));
        }
    };

    emit_band(0.0, top_band);
    emit_band(h - bottom_band, h);

    out
}

/// Horizontal inset of a rounded edge at height `y` (`bottom_start = h - bottom_r`).
fn corner_inset(y: f32, top_r: f32, bottom_start: f32, bottom_r: f32) -> f32 {
    if y < top_r {
        let dy = top_r - y;
        top_r - (top_r * top_r - dy * dy).max(0.0).sqrt()
    } else if bottom_r > 0.0 && y > bottom_start {
        let dy = y - bottom_start;
        bottom_r - (bottom_r * bottom_r - dy * dy).max(0.0).sqrt()
    } else {
        0.0
    }
}

/// Round-to-nearest on all four edges so adjacent slabs tile without gaps
/// (inward rounding would drop sub-pixel slabs). `None` when the rounding
/// collapses the rectangle to nothing.
#[allow(clippy::cast_possible_truncation)] // rounded pixel coordinates
fn to_blur_rect(b: Rectangle) -> Option<BlurRect> {
    let x0 = b.x.round() as i32;
    let y0 = b.y.round() as i32;
    let x1 = (b.x + b.width).round() as i32;
    let y1 = (b.y + b.height).round() as i32;
    (x1 > x0 && y1 > y0).then_some(BlurRect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
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
    fn zero_size_is_empty() {
        assert!(
            rounded_rect_to_blur_rects(rect(0.0, 0.0, 0.0, 10.0), Radius::from(4.0)).is_empty()
        );
        assert!(
            rounded_rect_to_blur_rects(rect(0.0, 0.0, 10.0, 0.0), Radius::from(4.0)).is_empty()
        );
    }

    #[test]
    fn no_radius_is_bounding_box() {
        let out = rounded_rect_to_blur_rects(rect(5.0, 7.0, 20.0, 12.0), Radius::from(0.0));
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0],
            BlurRect {
                x: 5,
                y: 7,
                width: 20,
                height: 12
            }
        );
    }

    #[test]
    fn rounded_stays_inscribed_within_bounds() {
        let bounds = rect(0.0, 0.0, 100.0, 40.0);
        let out = rounded_rect_to_blur_rects(bounds, Radius::from(12.0));
        assert!(out.len() > 1, "expected tessellation into multiple rects");
        for r in &out {
            assert!(r.x >= 0 && r.y >= 0, "rect leaks past top/left: {r:?}");
            assert!(
                (r.x + r.width) <= 100 && (r.y + r.height) <= 40,
                "rect leaks past bottom/right: {r:?}"
            );
            assert!(r.width > 0 && r.height > 0);
        }
    }

    #[test]
    fn corner_pixels_are_not_covered() {
        // With rounding, the extreme corners must be left un-blurred (inscribed).
        let bounds = rect(0.0, 0.0, 100.0, 40.0);
        let out = rounded_rect_to_blur_rects(bounds, Radius::from(12.0));
        let covers = |px: i32, py: i32| {
            out.iter()
                .any(|r| px >= r.x && px < r.x + r.width && py >= r.y && py < r.y + r.height)
        };
        assert!(!covers(0, 0), "top-left corner should be inscribed");
        assert!(!covers(99, 0), "top-right corner should be inscribed");
        assert!(!covers(0, 39), "bottom-left corner should be inscribed");
        assert!(!covers(99, 39), "bottom-right corner should be inscribed");
        // ...but the center is always covered.
        assert!(covers(50, 20), "center should be blurred");
    }

    #[test]
    fn rounded_respects_a_non_zero_origin() {
        // The origin has to be carried through every slab, not just the box.
        let bounds = rect(37.0, 12.0, 100.0, 40.0);
        let out = rounded_rect_to_blur_rects(bounds, Radius::from(12.0));
        assert!(out.len() > 1);
        for r in &out {
            assert!(r.x >= 37 && r.y >= 12, "rect leaks past top/left: {r:?}");
            assert!(
                (r.x + r.width) <= 137 && (r.y + r.height) <= 52,
                "rect leaks past bottom/right: {r:?}"
            );
        }
        let covers = |px: i32, py: i32| {
            out.iter()
                .any(|r| px >= r.x && px < r.x + r.width && py >= r.y && py < r.y + r.height)
        };
        assert!(covers(87, 32), "center should be blurred");
        assert!(!covers(37, 12), "top-left corner should be inscribed");
    }

    #[test]
    fn radius_larger_than_half_the_shortest_side_is_clamped() {
        // A pill: without clamping the top and bottom bands would overlap.
        let bounds = rect(0.0, 0.0, 100.0, 40.0);
        let out = rounded_rect_to_blur_rects(bounds, Radius::from(500.0));
        assert!(!out.is_empty());
        for r in &out {
            assert!(r.x >= 0 && r.y >= 0);
            assert!((r.x + r.width) <= 100 && (r.y + r.height) <= 40);
        }
        let covers = |px: i32, py: i32| {
            out.iter()
                .any(|r| px >= r.x && px < r.x + r.width && py >= r.y && py < r.y + r.height)
        };
        assert!(covers(50, 20), "center should be blurred");
        assert!(!covers(0, 0), "clamped corner should still be inscribed");
        // Widest point of the pill: the full width is reached at mid-height.
        assert!(
            covers(0, 20) && covers(99, 20),
            "waist should be full width"
        );
    }

    #[test]
    fn slabs_tile_without_gaps() {
        let out = rounded_rect_to_blur_rects(rect(0.0, 0.0, 100.0, 40.0), Radius::from(12.0));
        let covers = |px: i32, py: i32| {
            out.iter()
                .any(|r| px >= r.x && px < r.x + r.width && py >= r.y && py < r.y + r.height)
        };
        // The centre line crosses every band, so a dropped slab shows as a hole.
        for y in 0..40 {
            assert!(covers(50, y), "horizontal band missing at y={y}");
        }
    }

    #[test]
    fn per_corner_square_bottom_keeps_full_width_edge() {
        // Rounded only on top: bottom edge should reach the full width/height.
        let bounds = rect(0.0, 0.0, 100.0, 40.0);
        let radius = Radius {
            top_left: 12.0,
            top_right: 12.0,
            bottom_right: 0.0,
            bottom_left: 0.0,
        };
        let out = rounded_rect_to_blur_rects(bounds, radius);
        let bottom_reached = out.iter().any(|r| r.y + r.height >= 40);
        let full_width = out.iter().any(|r| r.x == 0 && r.width == 100);
        assert!(
            bottom_reached,
            "square bottom edge should reach full height"
        );
        assert!(
            full_width,
            "a full-width rect should exist for the straight body"
        );
        // The square bottom corners must be covered (no inscribing there).
        let covers = |px: i32, py: i32| {
            out.iter()
                .any(|r| px >= r.x && px < r.x + r.width && py >= r.y && py < r.y + r.height)
        };
        assert!(covers(0, 39), "square bottom-left corner should be blurred");
        assert!(
            covers(99, 39),
            "square bottom-right corner should be blurred"
        );
    }
}
