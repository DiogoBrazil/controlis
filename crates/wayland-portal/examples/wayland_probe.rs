//! Standalone probe for the Wayland portal backend.
//!
//! Run on a Wayland desktop to validate the consent flow, capture, and input:
//!
//! ```sh
//! cargo run -p wayland-portal --features enabled --example wayland_probe
//! ```
//!
//! It creates the combined ScreenCast + RemoteDesktop portal session (you will see
//! the system consent dialog), prints the granted monitor, waits for one captured
//! frame, and nudges the pointer to the screen center as an input smoke test.

use std::time::Duration;

use input::InputInjector;
use protocol::{KeyCode, MouseButton, PointerAction};
use wayland_portal::PortalHandle;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let handle = runtime.block_on(PortalHandle::new(runtime.handle().clone()))?;

    let monitor = handle.monitor().clone();
    println!(
        "granted monitor: {} {}x{}",
        monitor.name, monitor.width_px, monitor.height_px
    );

    let mut capturer = handle.capturer();
    let mut injector = handle.injector();

    use capture::ScreenCapturer;
    match capturer.capture(monitor.id) {
        Ok(frame) => println!(
            "captured a frame: {}x{} ({} bytes)",
            frame.width,
            frame.height,
            frame.data.len()
        ),
        Err(e) => println!("capture failed: {e}"),
    }

    // Input smoke test (libei/EIS): the ei device negotiates asynchronously after
    // ConnectToEIS, so move the cursor in a slow circle for a few seconds — early
    // commands are queued and applied once the device resumes. Watch the cursor.
    println!("moving the cursor for ~4s — watch the pointer…");
    let cx = monitor.width_px as f64 / 2.0;
    let cy = monitor.height_px as f64 / 2.0;
    let radius = (monitor.height_px as f64 / 4.0).max(50.0);
    for step in 0..80 {
        let angle = step as f64 * std::f64::consts::TAU / 40.0;
        let x = (cx + radius * angle.cos()) as i32;
        let y = (cy + radius * angle.sin()) as i32;
        injector.move_pointer(x, y)?;
        std::thread::sleep(Duration::from_millis(50));
    }

    let center = (cx as i32, cy as i32);
    injector.move_pointer(center.0, center.1)?;
    injector.mouse_button(MouseButton::Left, PointerAction::Press)?;
    injector.mouse_button(MouseButton::Left, PointerAction::Release)?;
    injector.release_all()?;
    println!("sent a circle of moves + a click at the center");

    // Keyboard test: focus a text field to see it typed.
    println!("typing 'controlis' in 3s — focus a text field NOW…");
    std::thread::sleep(Duration::from_secs(3));
    for ch in "controlis ".chars() {
        injector.key(KeyCode::Unicode(ch), PointerAction::Press)?;
        injector.key(KeyCode::Unicode(ch), PointerAction::Release)?;
        std::thread::sleep(Duration::from_millis(80));
    }
    injector.release_all()?;
    println!("sent the text 'controlis '");

    std::thread::sleep(Duration::from_millis(300));
    println!("probe done");
    Ok(())
}
