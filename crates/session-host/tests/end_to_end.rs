//! Full-cycle test: capture -> encode -> transport -> decode -> display, plus
//! input flowing viewer -> host -> injector. Uses the synthetic capturer and a
//! mock injector so it runs headless with no display or OS input access.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use capture::SyntheticCapturer;
use input::{InputError, InputInjector};
use protocol::{ControlMessage, KeyCode, PointerAction};
use session_host::{start, HostConfig, HostController, HostEvent};
use session_viewer::ViewerEvent;
use tokio::time::timeout;
use transport::HostIdentity;

/// Records the input events it receives so the test can assert on them.
#[derive(Default)]
struct MockInjector {
    log: Arc<Mutex<Vec<String>>>,
}

impl InputInjector for MockInjector {
    fn move_pointer(&mut self, x: i32, y: i32) -> Result<(), InputError> {
        self.log.lock().unwrap().push(format!("move {x},{y}"));
        Ok(())
    }
    fn mouse_button(&mut self, button: protocol::MouseButton, action: PointerAction) -> Result<(), InputError> {
        self.log.lock().unwrap().push(format!("button {button:?} {action:?}"));
        Ok(())
    }
    fn mouse_wheel(&mut self, dx: f32, dy: f32) -> Result<(), InputError> {
        self.log.lock().unwrap().push(format!("wheel {dx},{dy}"));
        Ok(())
    }
    fn key(&mut self, key: KeyCode, action: PointerAction) -> Result<(), InputError> {
        self.log.lock().unwrap().push(format!("key {key:?} {action:?}"));
        Ok(())
    }
    fn text(&mut self, text: &str) -> Result<(), InputError> {
        self.log.lock().unwrap().push(format!("text {text}"));
        Ok(())
    }
    fn release_all(&mut self) -> Result<(), InputError> {
        self.log.lock().unwrap().push("release_all".into());
        Ok(())
    }
}

async fn wait_for_code(controller: &mut HostController) -> String {
    loop {
        match timeout(Duration::from_secs(5), controller.next_event()).await {
            Ok(Some(HostEvent::CodeReady(code))) => return code,
            Ok(Some(_)) => continue,
            _ => panic!("host never produced a session code"),
        }
    }
}

#[tokio::test]
async fn full_cycle_streams_frames_and_applies_input() {
    let injected = Arc::new(Mutex::new(Vec::<String>::new()));
    let injected_for_factory = injected.clone();

    // Preferring the best compiled-in codec means this test streams H.264 when
    // the workspace is built with the `h264` feature, JPEG tiles otherwise.
    let config = HostConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        require_manual_approval: false,
        codec: codec::preferred_codec(),
        target_fps: 30,
        advertised_ip: Some(std::net::Ipv4Addr::LOCALHOST),
    };
    let identity = HostIdentity::generate().unwrap();

    let mut controller = start(
        config,
        identity,
        Arc::new(|| Ok(Box::new(SyntheticCapturer::new(320, 240)))),
        Arc::new(move || {
            Ok(Box::new(MockInjector {
                log: injected_for_factory.clone(),
            }))
        }),
    )
    .unwrap();

    let code = wait_for_code(&mut controller).await;
    // The access code is self-contained: the viewer derives the host address
    // from it instead of being told the IP separately.
    let parsed = security::ConnectCode::parse(&code).expect("code decodes");
    let addr: std::net::SocketAddr = parsed.addr().into();
    assert_eq!(addr, controller.local_addr());

    // Viewer connects and authenticates.
    let mut viewer = timeout(Duration::from_secs(5), session_viewer::connect(addr, code, None))
        .await
        .expect("connect timed out")
        .expect("connect failed");

    // A monitor list arrived with the synthetic monitor.
    assert_eq!(viewer.monitors().len(), 1);
    assert_eq!(viewer.monitors()[0].width_px, 320);
    assert_eq!(viewer.monitors()[0].origin_x, 0);

    // First decoded frame arrives.
    let got_frame = loop {
        match timeout(Duration::from_secs(5), viewer.next_event()).await {
            Ok(Some(ViewerEvent::FrameReady)) => break true,
            Ok(Some(_)) => continue,
            _ => break false,
        }
    };
    assert!(got_frame, "viewer never received a frame");
    let frame = viewer.frame_slot().lock().unwrap().clone().expect("frame present");
    assert_eq!(frame.width, 320);
    assert_eq!(frame.height, 240);

    // Send input and confirm it reaches the host's injector.
    viewer.send_input(ControlMessage::MouseMove { x_norm: 0.5, y_norm: 0.5 });
    viewer.send_input(ControlMessage::KeyEvent {
        key: KeyCode::Unicode('a'),
        action: PointerAction::Press,
    });
    viewer.send_input(ControlMessage::Text { text: "ação çê".into() });

    let mut saw_move = false;
    let mut saw_key = false;
    let mut saw_text = false;
    for _ in 0..50 {
        {
            let log = injected.lock().unwrap();
            saw_move = log.iter().any(|e| e.starts_with("move"));
            saw_key = log.iter().any(|e| e.contains("key"));
            saw_text = log.iter().any(|e| e == "text ação çê");
        }
        if saw_move && saw_key && saw_text {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(saw_move, "pointer move was not injected");
    assert!(saw_key, "key event was not injected");
    assert!(saw_text, "typed text was not injected");

    // Ending the session must release all held input on the host.
    controller.command(session_host::HostCommand::Stop);
    let mut released = false;
    for _ in 0..50 {
        if injected.lock().unwrap().iter().any(|e| e == "release_all") {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(released, "held input was not released on disconnect");
}

#[tokio::test]
async fn switching_monitor_changes_frame_size() {
    let config = HostConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        require_manual_approval: false,
        codec: codec::preferred_codec(),
        target_fps: 30,
        advertised_ip: Some(std::net::Ipv4Addr::LOCALHOST),
    };
    let identity = HostIdentity::generate().unwrap();

    let mut controller = start(
        config,
        identity,
        Arc::new(|| Ok(Box::new(SyntheticCapturer::from_layouts(&[(320, 240), (640, 480)])))),
        Arc::new(|| Ok(Box::new(MockInjector::default()))),
    )
    .unwrap();

    let addr = controller.local_addr();
    let code = wait_for_code(&mut controller).await;

    let mut viewer = timeout(Duration::from_secs(5), session_viewer::connect(addr, code, None))
        .await
        .expect("connect timed out")
        .expect("connect failed");
    assert_eq!(viewer.monitors().len(), 2);

    // Wait for the first monitor's frame (320x240).
    let first_size = wait_for_frame_size(&mut viewer).await;
    assert_eq!(first_size, (320, 240));

    // Switch to the second monitor and expect its size (640x480).
    viewer.select_monitor(1);
    let mut switched = None;
    for _ in 0..100 {
        let slot = viewer.frame_slot();
        let size = slot.lock().unwrap().as_ref().map(|f| (f.width, f.height));
        if size == Some((640, 480)) {
            switched = size;
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(switched, Some((640, 480)), "monitor switch did not change frame size");
}

async fn wait_for_frame_size(viewer: &mut session_viewer::ViewerHandle) -> (u32, u32) {
    for _ in 0..100 {
        if let Some(frame) = viewer.frame_slot().lock().unwrap().as_ref() {
            return (frame.width, frame.height);
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("no frame arrived");
}
