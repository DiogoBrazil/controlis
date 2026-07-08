use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use codec::RgbaFrame;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::Pod;

use crate::PortalError;

/// Handle that stops the PipeWire loop and joins its thread.
pub struct CaptureStop {
    quit: pw::channel::Sender<()>,
    join: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for CaptureStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CaptureStop")
    }
}

impl CaptureStop {
    pub fn stop(mut self) {
        let _ = self.quit.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct StreamData {
    frame: Arc<Mutex<Option<RgbaFrame>>>,
    format: VideoInfoRaw,
}

/// Starts the PipeWire loop on a dedicated thread, consuming the portal stream
/// `node_id` reached through the portal-provided `fd`, and publishing decoded RGBA
/// frames into `frame`.
pub fn spawn_capture(
    fd: OwnedFd,
    node_id: u32,
    frame: Arc<Mutex<Option<RgbaFrame>>>,
) -> Result<CaptureStop, PortalError> {
    let (quit_tx, quit_rx) = pw::channel::channel::<()>();

    let join = std::thread::Builder::new()
        .name("controlis-pipewire".into())
        .spawn(move || {
            if let Err(e) = run_loop(fd, node_id, frame, quit_rx) {
                tracing::error!("pipewire capture loop failed: {e}");
            }
        })
        .map_err(|e| PortalError::PipeWire(e.to_string()))?;

    Ok(CaptureStop {
        quit: quit_tx,
        join: Some(join),
    })
}

fn run_loop(
    fd: OwnedFd,
    node_id: u32,
    frame: Arc<Mutex<Option<RgbaFrame>>>,
    quit_rx: pw::channel::Receiver<()>,
) -> Result<(), PortalError> {
    pw::init();

    let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(pw_err)?;
    let context = pw::context::ContextRc::new(&main_loop, None).map_err(pw_err)?;
    let core = context.connect_fd_rc(fd, None).map_err(pw_err)?;

    let data = StreamData {
        frame,
        format: VideoInfoRaw::default(),
    };

    let stream = pw::stream::StreamBox::new(
        &core,
        "controlis-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(pw_err)?;

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, data, id, param| {
            let Some(param) = param else { return };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = pw::spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
                return;
            }
            let _ = data.format.parse(param);
        })
        .process(|stream, data| {
            if let Some(mut buffer) = stream.dequeue_buffer() {
                publish_frame(data, &mut buffer);
            }
        })
        .register()
        .map_err(pw_err)?;

    let values = format_pod_bytes();
    let mut params = [Pod::from_bytes(&values).expect("valid serialized pod")];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(pw_err)?;

    // Quit the loop when the stop signal arrives from another thread.
    let main_loop_weak = main_loop.downgrade();
    let _quit = quit_rx.attach(main_loop.loop_(), move |_| {
        if let Some(main_loop) = main_loop_weak.upgrade() {
            main_loop.quit();
        }
    });

    main_loop.run();
    Ok(())
}

fn publish_frame(data: &mut StreamData, buffer: &mut pw::buffer::Buffer) {
    let datas = buffer.datas_mut();
    if datas.is_empty() {
        return;
    }
    let plane = &mut datas[0];
    let stride = plane.chunk().stride() as usize;
    let width = data.format.size().width as usize;
    let height = data.format.size().height as usize;
    if width == 0 || height == 0 || stride < width * 4 {
        return;
    }
    let format = data.format.format();
    let Some(bytes) = plane.data() else { return };
    if bytes.len() < stride * height {
        return;
    }

    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        let src_row = &bytes[y * stride..y * stride + width * 4];
        let dst_row = &mut rgba[y * width * 4..(y + 1) * width * 4];
        convert_row(src_row, dst_row, format);
    }

    if let Ok(new_frame) = RgbaFrame::new(width as u32, height as u32, rgba) {
        if let Ok(mut slot) = data.frame.lock() {
            *slot = Some(new_frame);
        }
    }
}

/// Converts one row from the negotiated pixel format into RGBA.
fn convert_row(src: &[u8], dst: &mut [u8], format: VideoFormat) {
    let swap_rb = matches!(format, VideoFormat::BGRA | VideoFormat::BGRx);
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        if swap_rb {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
        } else {
            d[0] = s[0];
            d[1] = s[1];
            d[2] = s[2];
        }
        d[3] = match format {
            VideoFormat::RGBA | VideoFormat::BGRA => s[3],
            _ => 255,
        };
    }
}

/// Serializes the EnumFormat parameter advertising the raw formats we can consume.
fn format_pod_bytes() -> Vec<u8> {
    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pw::spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pw::spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::RGBA,
            VideoFormat::RGBA,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::BGRx
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle { width: 1920, height: 1080 },
            pw::spa::utils::Rectangle { width: 1, height: 1 },
            pw::spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 30, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction { num: 240, denom: 1 }
        ),
    );

    pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .expect("serialize format pod")
    .0
    .into_inner()
}

fn pw_err<E: std::fmt::Display>(e: E) -> PortalError {
    PortalError::PipeWire(e.to_string())
}
