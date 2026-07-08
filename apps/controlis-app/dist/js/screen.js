// Browser-side adapter for the remote-screen canvas: receives JPEG frames over
// a binary Tauri channel, draws them letterboxed, and forwards pointer/keyboard
// input back to the viewer session.

const core = () => window.__TAURI__.core;

const NAMED_KEYS = new Set([
  "Enter", "Escape", "Backspace", "Tab", "Delete", "Home", "End",
  "PageUp", "PageDown", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
  "Shift", "Control", "Alt", "AltGraph", "Meta", "OS", "CapsLock",
  "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
]);

/**
 * Connects the viewer session and wires the canvas.
 * @param {HTMLCanvasElement} canvas target canvas
 * @param {string} code access code typed by the user
 * @param {string|null} addrOverride optional manual IP:port
 * @param {(event:object)=>void} onEvent status events (resized/disconnected/...)
 * @returns handle with selectMonitor()/disconnect()/dispose(); resolves after auth
 */
export async function connectScreen(canvas, code, addrOverride, onEvent) {
  const ctx = canvas.getContext("2d");
  let disposed = false;
  let remoteSize = null; // {w, h} of the last frame

  const Channel = core().Channel;

  const frames = new Channel();
  frames.onmessage = async (data) => {
    if (disposed) return;
    try {
      const blob = new Blob([data], { type: "image/jpeg" });
      const bitmap = await createImageBitmap(blob);
      remoteSize = { w: bitmap.width, h: bitmap.height };
      draw(bitmap);
      bitmap.close();
    } catch (e) {
      console.warn("frame decode failed", e);
    }
  };

  const events = new Channel();
  events.onmessage = (msg) => {
    if (!disposed) onEvent(msg);
  };

  function draw(bitmap) {
    // Match the canvas backing store to its CSS size for crisp scaling.
    const rect = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const bw = Math.max(1, Math.round(rect.width * dpr));
    const bh = Math.max(1, Math.round(rect.height * dpr));
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
    }
    const fit = fitRect(bitmap.width, bitmap.height, canvas.width, canvas.height);
    ctx.fillStyle = "#0b0e12";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(bitmap, fit.x, fit.y, fit.w, fit.h);
  }

  function fitRect(srcW, srcH, dstW, dstH) {
    const scale = Math.min(dstW / srcW, dstH / srcH);
    const w = srcW * scale;
    const h = srcH * scale;
    return { x: (dstW - w) / 2, y: (dstH - h) / 2, w, h };
  }

  // Maps a mouse event to coordinates normalized to the remote image (the
  // letterboxed area of the canvas), or null when outside the image.
  function normalized(e) {
    if (!remoteSize) return null;
    const rect = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const fit = fitRect(remoteSize.w, remoteSize.h, rect.width * dpr, rect.height * dpr);
    const x = ((e.clientX - rect.left) * dpr - fit.x) / fit.w;
    const y = ((e.clientY - rect.top) * dpr - fit.y) / fit.h;
    if (x < 0 || x > 1 || y < 0 || y > 1) return null;
    return { x, y };
  }

  const send = (event) => {
    core().invoke("viewer_input", { event }).catch(() => {});
  };

  const onPointerMove = (e) => {
    const pos = normalized(e);
    if (pos) send({ type: "move", x: pos.x, y: pos.y });
  };
  const onPointerDown = (e) => {
    canvas.focus();
    e.preventDefault();
    const pos = normalized(e);
    if (pos) send({ type: "move", x: pos.x, y: pos.y });
    send({ type: "button", button: e.button, pressed: true });
  };
  const onPointerUp = (e) => {
    e.preventDefault();
    send({ type: "button", button: e.button, pressed: false });
  };
  const onWheel = (e) => {
    e.preventDefault();
    // Browser deltaY is positive scrolling down; the wire wants positive = up.
    send({ type: "wheel", dx: -e.deltaX / 60, dy: -e.deltaY / 60 });
  };
  const onContextMenu = (e) => e.preventDefault();

  const onKeyDown = (e) => {
    e.preventDefault();
    if (NAMED_KEYS.has(e.key)) {
      send({ type: "key", key: e.key, pressed: true });
    } else if (e.key.length === 1) {
      if (e.ctrlKey || e.altKey || e.metaKey) {
        // Shortcut chord: the physical key matters (Ctrl+C etc.).
        send({ type: "key", key: e.key, pressed: true });
      } else {
        // Plain typing travels as text: exact characters, layout-independent.
        send({ type: "text", text: e.key });
      }
    }
  };
  const onKeyUp = (e) => {
    e.preventDefault();
    if (NAMED_KEYS.has(e.key) || (e.key.length === 1 && (e.ctrlKey || e.altKey || e.metaKey))) {
      send({ type: "key", key: e.key, pressed: false });
    }
  };
  const onBlur = () => {
    // Never leave modifiers stuck on the host when focus moves away.
    for (const key of ["Shift", "Control", "Alt", "Meta"]) {
      send({ type: "key", key, pressed: false });
    }
  };

  canvas.addEventListener("mousemove", onPointerMove);
  canvas.addEventListener("mousedown", onPointerDown);
  canvas.addEventListener("mouseup", onPointerUp);
  canvas.addEventListener("wheel", onWheel, { passive: false });
  canvas.addEventListener("contextmenu", onContextMenu);
  canvas.addEventListener("keydown", onKeyDown);
  canvas.addEventListener("keyup", onKeyUp);
  canvas.addEventListener("blur", onBlur);

  function dispose() {
    if (disposed) return;
    disposed = true;
    canvas.removeEventListener("mousemove", onPointerMove);
    canvas.removeEventListener("mousedown", onPointerDown);
    canvas.removeEventListener("mouseup", onPointerUp);
    canvas.removeEventListener("wheel", onWheel);
    canvas.removeEventListener("contextmenu", onContextMenu);
    canvas.removeEventListener("keydown", onKeyDown);
    canvas.removeEventListener("keyup", onKeyUp);
    canvas.removeEventListener("blur", onBlur);
  }

  const info = await core().invoke("connect_viewer", {
    code,
    addrOverride: addrOverride || null,
    onEvent: events,
    onFrame: frames,
  });

  canvas.focus();

  return {
    info,
    selectMonitor(id) {
      core().invoke("viewer_select_monitor", { monitorId: id }).catch(() => {});
    },
    disconnect() {
      dispose();
      core().invoke("viewer_disconnect", {}).catch(() => {});
    },
    dispose,
  };
}
