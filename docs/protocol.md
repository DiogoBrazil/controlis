# Protocolo Controlis (v1)

Fonte de verdade das mensagens trocadas entre host e viewer. Implementado em
`crates/protocol`.

## Transporte e framing

- Base: QUIC (TLS 1.3). Uma conexão por sessão. `MonitorInfo` inclui `origin_x/origin_y` (posição do
monitor no desktop virtual, em pixels físicos) para mapear coordenadas
normalizadas ao monitor correto em setups multi-monitor.
- **Stream de controle**: bidirecional, confiável e ordenado. Carrega
  `ControlMessage` com framing *length-delimited*: prefixo `u32` little-endian
  com o tamanho do corpo, seguido do corpo serializado com `postcard`.
- **Streams de mídia**: unidirecionais (host → viewer), um `MediaMessage` por
  stream. O limite do stream já delimita a mensagem.

Serialização: `serde` + `postcard` (binário compacto, Rust puro). JSON só para
depuração. Payloads binários (quadros) usam `serde_bytes` para evitar overhead.

## Versionamento

`Hello.protocol_version` (`u16`, atualmente `3`) é trocado no início. Versão
incompatível é rejeitada com `Error` e a conexão é encerrada.

Histórico:
- v2 adicionou `VideoCodec::H264` (muda a forma no fio de `Hello` e dos
  quadros de mídia).
- v3 adicionou `Text { text }`: digitação viaja como texto e o host injeta os
  caracteres exatos, independente de layout/shift (no Windows, via `SendInput`
  + `KEYEVENTF_UNICODE`). `KeyEvent` passa a ser usado apenas para teclas
  nomeadas e atalhos com modificador.

## Handshake e autenticação

```
viewer → host : Hello { protocol_version, app_version, role, supported_codecs }
viewer → host : AuthRequest { session_code }
host  → viewer: AuthResponse(PendingApproval)      # se aprovação manual exigida
host  → viewer: AuthResponse(Accepted { session_id } | Rejected { reason })
host  → viewer: MonitorList { monitors: [MonitorInfo] }
```

Regras no host:
- Código verificado em tempo constante; falha incrementa o backoff por IP.
- IP em backoff recebe `Rejected` imediato.
- Com aprovação manual, o host aguarda a decisão do usuário antes de aceitar.

## Controle (após aceite)

| Mensagem | Direção | Observação |
|---|---|---|
| `MouseMove { x_norm, y_norm }` | viewer → host | coordenadas normalizadas 0..1 no monitor ativo |
| `MouseButton { button, action }` | viewer → host | `Left/Right/Middle`, `Press/Release` |
| `MouseWheel { delta_x, delta_y }` | viewer → host | positivo y = cima |
| `KeyEvent { key, action }` | viewer → host | teclas nomeadas e atalhos: `KeyCode` nomeado ou `Unicode(char)` com modificador |
| `Text { text }` | viewer → host | digitação: caracteres imprimíveis injetados literalmente no host (independe de layout/shift) |
| `SelectMonitor { monitor_id }` | viewer → host | troca o monitor capturado ao vivo; host força keyframe e envia `Resize` |
| `Resize { width_px, height_px }` | host → viewer | mudança de resolução do monitor ativo |
| `Ping/Pong { nonce }` | ambos | verificação de vida |
| `Disconnect { reason }` | ambos | encerramento limpo |
| `Error { code, message }` | ambos | erro de protocolo |

Coordenadas normalizadas resolvem, por construção, diferenças de resolução e de
DPI: o host converte `0..1` para os pixels físicos do seu monitor.

## Mídia

```
MediaMessage::ScreenFrame {
  monitor_id, frame_id, codec, encoding, width_px, height_px, tiles, payload
}
```

- `codec`: `H264` (preferido quando ambos suportam), `JpegTiles` (baseline
  obrigatório) ou `RawRgba` (apenas teste em LAN).
- `encoding = Full`: `payload` cobre o quadro inteiro; `tiles` vazio.
- `encoding = TileDelta`: `payload` é a concatenação de blocos JPEG só dos tiles
  alterados, cada um prefixado por `u32` com seu tamanho, na ordem de `tiles`.

O primeiro quadro (e qualquer mudança de resolução) é enviado como `Full`; os
demais como `TileDelta`, transmitindo apenas os blocos que mudaram.

### Negociação de codec

O viewer anuncia `supported_codecs` no `Hello` (melhor primeiro). O host escolhe
seu codec preferido se o viewer o suportar; senão, o primeiro codec (na ordem de
preferência do host) que ambos entendem; em último caso, `JpegTiles`. O codec
efetivo vai marcado em cada `ScreenFrame`, então o viewer não precisa de aviso
prévio.

### H.264

Com `codec = H264`, cada `ScreenFrame` carrega um trecho Annex-B do bitstream
(um quadro codificado por mensagem); `encoding` é sempre `Full` e `tiles` fica
vazio — o próprio stream H.264 distingue keyframe de delta. A ordem é garantida
porque o viewer aceita os streams unidirecionais em ordem de criação. Na troca
de monitor o host força um IDR; dimensões ímpares são cortadas para pares
(exigência do YUV 4:2:0). Encoder/decoder: OpenH264 (`openh264`, compilado do
fonte), perfil de uso `ScreenContentRealTime`, controle de taxa por bitrate
(~4 Mbps).
