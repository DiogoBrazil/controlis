use codec::RgbaFrame;
use protocol::MonitorInfo;

use crate::{CaptureError, ScreenCapturer};

/// One virtual monitor of the synthetic capturer.
#[derive(Debug, Clone)]
struct VirtualMonitor {
    info: MonitorInfo,
    tick: u64,
}

/// A capturer that renders moving patterns instead of a real screen.
///
/// Lets the full capture -> encode -> transport -> decode -> display pipeline run
/// on machines with no display and no capture system libraries (CI, containers),
/// gives deterministic content for tests, and exposes several virtual monitors so
/// multi-monitor selection can be exercised headless.
#[derive(Debug)]
pub struct SyntheticCapturer {
    monitors: Vec<VirtualMonitor>,
}

impl SyntheticCapturer {
    /// A single virtual monitor of the given size.
    pub fn new(width: u32, height: u32) -> Self {
        Self::from_layouts(&[(width, height)])
    }

    /// Several virtual monitors laid out left to right; the first is primary.
    pub fn from_layouts(sizes: &[(u32, u32)]) -> Self {
        let mut monitors = Vec::with_capacity(sizes.len());
        let mut origin_x = 0i32;
        for (index, (width, height)) in sizes.iter().enumerate() {
            monitors.push(VirtualMonitor {
                info: MonitorInfo {
                    id: index as u32,
                    name: format!("Synthetic {}", index + 1),
                    origin_x,
                    origin_y: 0,
                    width_px: *width,
                    height_px: *height,
                    is_primary: index == 0,
                },
                tick: 0,
            });
            origin_x += *width as i32;
        }
        Self { monitors }
    }

    fn monitor_mut(&mut self, id: u32) -> Option<&mut VirtualMonitor> {
        self.monitors.iter_mut().find(|m| m.info.id == id)
    }
}

impl Default for SyntheticCapturer {
    fn default() -> Self {
        Self::from_layouts(&[(1280, 720), (1024, 768)])
    }
}

impl ScreenCapturer for SyntheticCapturer {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        Ok(self.monitors.iter().map(|m| m.info.clone()).collect())
    }

    fn capture(&mut self, monitor_id: u32) -> Result<RgbaFrame, CaptureError> {
        let monitor = self
            .monitor_mut(monitor_id)
            .ok_or(CaptureError::MonitorNotFound(monitor_id))?;
        let (w, h) = (monitor.info.width_px, monitor.info.height_px);
        // Tint each monitor differently so a monitor switch is visually obvious.
        let tint = monitor.info.id;
        let mut data = vec![0u8; w as usize * h as usize * 4];

        let box_size = 80u32;
        let travel_x = w.saturating_sub(box_size).max(1);
        let bx = (monitor.tick as u32 * 7) % travel_x;
        let by = h.saturating_sub(box_size) / 2;

        for y in 0..h {
            for x in 0..w {
                let i = (y as usize * w as usize + x as usize) * 4;
                let in_box = x >= bx && x < bx + box_size && y >= by && y < by + box_size;
                if in_box {
                    data[i] = 240;
                    data[i + 1] = 80;
                    data[i + 2] = 40;
                } else {
                    data[i] = (x * 255 / w) as u8;
                    data[i + 1] = (y * 255 / h) as u8;
                    data[i + 2] = (64 + tint * 60).min(255) as u8;
                }
                data[i + 3] = 255;
            }
        }

        monitor.tick = monitor.tick.wrapping_add(1);
        Ok(RgbaFrame::new(w, h, data)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_reports_multiple_monitors_with_origins() {
        let cap = SyntheticCapturer::default();
        let monitors = cap.monitors().unwrap();
        assert_eq!(monitors.len(), 2);
        assert!(monitors[0].is_primary);
        assert_eq!(monitors[0].origin_x, 0);
        // Second monitor sits to the right of the first.
        assert_eq!(monitors[1].origin_x, monitors[0].width_px as i32);
    }

    #[test]
    fn captures_each_monitor_at_its_own_size() {
        let mut cap = SyntheticCapturer::from_layouts(&[(320, 240), (640, 480)]);
        let a = cap.capture(0).unwrap();
        let b = cap.capture(1).unwrap();
        assert_eq!((a.width, a.height), (320, 240));
        assert_eq!((b.width, b.height), (640, 480));
    }

    #[test]
    fn frames_change_between_captures() {
        let mut cap = SyntheticCapturer::new(320, 240);
        let a = cap.capture(0).unwrap();
        let b = cap.capture(0).unwrap();
        assert_ne!(a.data, b.data);
    }

    #[test]
    fn rejects_unknown_monitor() {
        let mut cap = SyntheticCapturer::new(320, 240);
        assert!(matches!(cap.capture(99), Err(CaptureError::MonitorNotFound(99))));
    }
}
