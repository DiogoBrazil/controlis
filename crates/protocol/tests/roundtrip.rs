use protocol::{
    decode, encode, negotiate_codec, AuthOutcome, ControlMessage, Hello, KeyCode, MediaMessage,
    MonitorInfo, MouseButton, PointerAction, Role, ScreenEncoding, TileRect, VideoCodec,
    PROTOCOL_VERSION,
};

fn roundtrip_control(msg: ControlMessage) {
    let framed = encode(&msg).unwrap();
    // Strip the 4-byte length prefix to get the body decode() expects.
    let body = &framed[4..];
    let decoded: ControlMessage = decode(body).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn control_messages_roundtrip() {
    roundtrip_control(ControlMessage::Hello(Hello {
        protocol_version: PROTOCOL_VERSION,
        app_version: "0.1.0".into(),
        role: Role::Viewer,
        supported_codecs: vec![VideoCodec::JpegTiles, VideoCodec::RawRgba],
    }));
    roundtrip_control(ControlMessage::AuthRequest {
        session_code: "K7QP-2M9X".into(),
    });
    roundtrip_control(ControlMessage::AuthResponse(AuthOutcome::Accepted {
        session_id: 12345,
    }));
    roundtrip_control(ControlMessage::AuthResponse(AuthOutcome::Rejected {
        reason: "invalid code".into(),
    }));
    roundtrip_control(ControlMessage::MonitorList {
        monitors: vec![MonitorInfo {
            id: 0,
            name: "eDP-1".into(),
            origin_x: 0,
            origin_y: 0,
            width_px: 1920,
            height_px: 1080,
            is_primary: true,
        }],
    });
    roundtrip_control(ControlMessage::MouseMove {
        x_norm: 0.5,
        y_norm: 0.25,
    });
    roundtrip_control(ControlMessage::MouseButton {
        button: MouseButton::Right,
        action: PointerAction::Press,
    });
    roundtrip_control(ControlMessage::MouseWheel {
        delta_x: -1.0,
        delta_y: 3.5,
    });
    roundtrip_control(ControlMessage::KeyEvent {
        key: KeyCode::Unicode('ç'),
        action: PointerAction::Press,
    });
    roundtrip_control(ControlMessage::KeyEvent {
        key: KeyCode::Function(5),
        action: PointerAction::Release,
    });
    roundtrip_control(ControlMessage::Text {
        text: "ação já çê !@# ABC 123".into(),
    });
    roundtrip_control(ControlMessage::Ping { nonce: u64::MAX });
    roundtrip_control(ControlMessage::Disconnect {
        reason: "user closed".into(),
    });
}

#[test]
fn codec_negotiation_prefers_common_codecs() {
    use VideoCodec::{JpegTiles, RawRgba, H264};

    // Preferred codec wins when both sides support it.
    assert_eq!(negotiate_codec(H264, &[H264, JpegTiles], &[H264, JpegTiles]), H264);
    // Viewer without H264 falls back to the best common codec.
    assert_eq!(negotiate_codec(H264, &[H264, JpegTiles], &[JpegTiles, RawRgba]), JpegTiles);
    // Host built without H264 ignores a viewer that offers it.
    assert_eq!(negotiate_codec(H264, &[JpegTiles, RawRgba], &[H264, JpegTiles]), JpegTiles);
    // No overlap still lands on the mandatory baseline.
    assert_eq!(negotiate_codec(H264, &[H264], &[RawRgba]), JpegTiles);
}

#[test]
fn media_frame_roundtrips_with_payload() {
    let msg = MediaMessage::ScreenFrame {
        monitor_id: 1,
        frame_id: 99,
        codec: VideoCodec::JpegTiles,
        encoding: ScreenEncoding::TileDelta,
        width_px: 1920,
        height_px: 1080,
        tiles: vec![
            TileRect { x: 0, y: 0, w: 64, h: 64 },
            TileRect { x: 64, y: 0, w: 64, h: 64 },
        ],
        payload: vec![1, 2, 3, 4, 5, 6, 7, 8],
    };
    let framed = encode(&msg).unwrap();
    let decoded: MediaMessage = decode(&framed[4..]).unwrap();
    assert_eq!(decoded, msg);
}
