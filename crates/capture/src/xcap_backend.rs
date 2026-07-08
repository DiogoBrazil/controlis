use codec::RgbaFrame;
use protocol::MonitorInfo;
use xcap::Monitor;

use crate::{CaptureError, ScreenCapturer};

/// Screen capture backed by the `xcap` crate.
#[derive(Debug, Default)]
pub struct XcapCapturer;

impl XcapCapturer {
    pub fn new() -> Self {
        Self
    }

    fn find(monitor_id: u32) -> Result<Monitor, CaptureError> {
        let monitors = Monitor::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
        for monitor in monitors {
            if monitor.id().map_err(|e| CaptureError::Backend(e.to_string()))? == monitor_id {
                return Ok(monitor);
            }
        }
        Err(CaptureError::MonitorNotFound(monitor_id))
    }
}

impl ScreenCapturer for XcapCapturer {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        let monitors = Monitor::all().map_err(|e| CaptureError::Backend(e.to_string()))?;
        if monitors.is_empty() {
            return Err(CaptureError::NoMonitors);
        }
        let mut infos = Vec::with_capacity(monitors.len());
        for monitor in monitors {
            let to_backend = |e: xcap::XCapError| CaptureError::Backend(e.to_string());
            infos.push(MonitorInfo {
                id: monitor.id().map_err(to_backend)?,
                name: monitor.name().map_err(to_backend)?,
                origin_x: monitor.x().map_err(to_backend)?,
                origin_y: monitor.y().map_err(to_backend)?,
                width_px: monitor.width().map_err(to_backend)?,
                height_px: monitor.height().map_err(to_backend)?,
                is_primary: monitor.is_primary().map_err(to_backend)?,
            });
        }
        Ok(infos)
    }

    fn capture(&mut self, monitor_id: u32) -> Result<RgbaFrame, CaptureError> {
        let monitor = Self::find(monitor_id)?;
        let image = monitor
            .capture_image()
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        let width = image.width();
        let height = image.height();
        let frame = RgbaFrame::new(width, height, image.into_raw())?;
        Ok(frame)
    }
}
