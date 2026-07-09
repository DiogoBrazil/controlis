# Arquitetura

## Threads e runtime

- `tokio` (multi-thread) cuida de rede e orquestração assíncrona.
- Captura e encode rodam em uma **thread dedicada** (trabalho bloqueante/CPU) e
  entregam `MediaMessage` a uma task async por canal.
- Injeção de input roda em outra **thread dedicada** (os handles do enigo não são
  `Send` e devem viver em uma única thread); recebe ações por canal.
- A UI nunca toca em rede/captura diretamente — só troca mensagens via os
  handles das sessões.

Esse desacoplamento mantém host, viewer, transporte, codec e UI independentes.

## UI (Tauri v2 + Leptos)

O app desktop (`apps/controlis-app`) é um webview Tauri com frontend Leptos
(CSR, compilado para wasm via Trunk). Dois workspaces: o crate `controlis-ui`
(frontend) e `src-tauri` (backend, que depende dos crates de sessão).

- **Comandos** (`src-tauri/src/commands/`): `start_host`/`host_command`/
  `stop_host` e `connect_viewer`/`viewer_input`/`viewer_select_monitor`/
  `viewer_disconnect`. O frontend chama via `window.__TAURI__.core.invoke`
  (`withGlobalTauri`).
- **Eventos** fluem por `tauri::ipc::Channel`: um canal JSON de status por
  sessão (eventos do host/viewer) e, no viewer, um **canal binário de frames**
  — o backend decodifica o vídeo da sessão como sempre (H.264/tiles), re-encoda
  cada quadro composto em JPEG (`codec::encode_rgba_to_jpeg`) numa thread
  dedicada e envia os bytes; o JS desenha no canvas via `createImageBitmap`
  (decodificação nativa do navegador), com letterbox.
- **Input**: `public/js/screen.js` captura pointer/teclado no canvas
  (coordenadas normalizadas à área útil da imagem) e invoca `viewer_input`;
  `src-tauri/src/input_map.rs` traduz para o protocolo — digitação vira `Text`,
  teclas nomeadas/modificadores/atalhos viram `KeyEvent` (o browser entrega
  keydown/keyup explícitos de Shift/Ctrl/…, sem diff de modificadores).
- **Ciclo de vida**: os pumps de evento são donos de `HostController`/
  `ViewerHandle`; os comandos usam senders clonados. `RunEvent::Exit` (fechar a
  janela ou Ctrl+C via `tokio::signal`) dropa as sessões e o keepalive do
  portal Wayland, preservando o teardown limpo (EIS disconnect + ack do portal).

## Fluxo do host (uma sessão por vez, no MVP)

```
HostListener.accept ─▶ handshake+auth ─▶ MonitorList
                                            │
        ┌───────────────────────────────────┼───────────────────────────────┐
        ▼                                    ▼                               ▼
 thread captura/encode            task envio de mídia              loop de controle (async)
 capturer→TileEncoder → canal ─▶  conn.send_media (uni stream)     recv input → canal ─▶ thread de injeção
```

Ao encerrar: sinaliza `stop`, fecha o canal de input (a thread de injeção chama
`release_all`, liberando teclas/botões), fecha a conexão e emite `SessionEnded`.

## Fluxo do viewer

```
connect ─▶ Hello+AuthRequest ─▶ aguarda Accepted ─▶ MonitorList
   │
   ├─ task de input:     input_rx → ControlSender.send
   ├─ task de controle:  ControlReceiver.recv (Resize/Disconnect/Error)
   └─ task de mídia:     conn.recv_media → TileDecoder → FrameSlot (a UI lê e faz upload da textura)
```

O canal de controle é dividido (`ControlChannel::split`) em metades de envio e
recepção independentes, pois input e mensagens do host precisam ser concorrentes.

## Backends atrás de traits

- `ScreenCapturer`: `SyntheticCapturer` (sempre disponível, para dev/teste) e
  `XcapCapturer` (feature `xcap-backend`).
- `InputInjector`: `EnigoInjector` (SendInput/XTest). Wayland (portal
  RemoteDesktop) entra depois, atrás do mesmo trait.
- `Encoder`/`Decoder` de tela: `VideoEncoder`/`VideoDecoder` despacham para
  `TileEncoder`/`TileDecoder` (baseline JPEG-tiles, sempre disponível) ou
  `H264Encoder`/`H264Decoder` (OpenH264, feature `h264`). O codec da sessão é
  negociado via `supported_codecs` no `Hello`, com fallback para JPEG-tiles; o
  decoder do viewer despacha pelo codec marcado em cada quadro.

## Segurança do transporte

Identidade do host = certificado self-signed **persistente** (`rcgen`, salvo em
`host_cert.der`/`host_key.der`). O viewer usa um verificador **trust-on-first-use**
que fixa a impressão digital SHA-256: primeiro contato registra; contatos
seguintes exigem a mesma, abortando o handshake em caso de divergência.
