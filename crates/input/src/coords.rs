//! Pure conversion between normalized wire coordinates and physical pixels.

/// Maps a normalized coordinate (`0.0..=1.0`) within a monitor to an absolute
/// desktop pixel, accounting for the monitor's top-left origin.
///
/// Inputs are clamped, so a hostile or out-of-range value can never index outside
/// the monitor. The result is always within `[origin, origin + size)`.
pub fn normalized_to_pixel(
    x_norm: f32,
    y_norm: f32,
    origin_x: i32,
    origin_y: i32,
    width: u32,
    height: u32,
) -> (i32, i32) {
    let x = axis(x_norm, width);
    let y = axis(y_norm, height);
    (origin_x + x, origin_y + y)
}

fn axis(norm: f32, size: u32) -> i32 {
    if size == 0 {
        return 0;
    }
    let clamped = if norm.is_nan() { 0.0 } else { norm.clamp(0.0, 1.0) };
    let last = size - 1;
    let scaled = (clamped * last as f32).round() as i64;
    scaled.clamp(0, last as i64) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn endpoints_map_to_first_and_last_pixel() {
        assert_eq!(normalized_to_pixel(0.0, 0.0, 0, 0, 1920, 1080), (0, 0));
        assert_eq!(normalized_to_pixel(1.0, 1.0, 0, 0, 1920, 1080), (1919, 1079));
    }

    #[test]
    fn origin_offsets_the_result() {
        assert_eq!(normalized_to_pixel(0.0, 0.0, 1920, 0, 1280, 1024), (1920, 0));
        assert_eq!(normalized_to_pixel(1.0, 1.0, 1920, 0, 1280, 1024), (1920 + 1279, 1023));
    }

    proptest! {
        #[test]
        fn always_inside_monitor_bounds(
            x_norm in any::<f32>(),
            y_norm in any::<f32>(),
            width in 1u32..8000,
            height in 1u32..8000,
            ox in -4000i32..4000,
            oy in -4000i32..4000,
        ) {
            let (x, y) = normalized_to_pixel(x_norm, y_norm, ox, oy, width, height);
            prop_assert!(x >= ox && x < ox + width as i32);
            prop_assert!(y >= oy && y < oy + height as i32);
        }
    }
}
