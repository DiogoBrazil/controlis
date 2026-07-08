# Plano Técnico — App de Acesso Remoto em Rust (codinome: "Controlis")

> Documento de planejamento. Nenhum código foi escrito ainda.
> Data: 2026-07-06. Aguarda aprovação das decisões da seção 12 antes de qualquer implementação.

---

## 1. Resumo da recomendação

| Decisão | Recomendação |
|---|---|
| Arquitetura | Binário único com modo `host` e `viewer`, conexão direta LAN no MVP, sem servidor externo |
| Linguagem | 100% Rust (exceção pontual: libjpeg-turbo/OpenH264 via bindings, opcional) |
| UI | `egui`/`eframe` |
| Transporte | QUIC via `quinn` (TLS 1.3 embutido via `rustls`) |
| Captura de tela | `xcap` (Windows GDI/WGC, Linux X11; Wayland em fase posterior via `ashpd` + PipeWire) |
| Injeção de input | `enigo` (SendInput no Windows, XTest no X11; Wayland via portal RemoteDesktop depois) |
| Codec inicial | Blocos com detecção de mudança (dirty tiles) + JPEG; vídeo real (OpenH264/VP8) em fase futura |
| Serialização | `serde` + `postcard`, com framing length-delimited; JSON opcional para debug |
| Persistência | Arquivo TOML para config + SQLite (`rusqlite`) para logs e dispositivos conhecidos |
| Autenticação | Código de sessão temporário + aprovação manual no host + TOFU de fingerprint do certificado |
| Plataforma piloto | Linux X11 primeiro (máquina de desenvolvimento), Windows logo em seguida |

A tese central: **todo o caminho crítico (captura → codec → transporte → exibição → input) tem crates Rust maduros o suficiente para um MVP sem sair do Rust**. Os dois pontos onde Rust puro é discutível — codec de vídeo e Wayland — ficam fora do MVP e têm plano claro (seção 3.6 e 11).

---

## 2. Arquitetura proposta

### 2.1 Modelo de aplicação: binário único, dois modos

Um único executável `controlis` com duas telas/modos (mesmo modelo do RustDesk):

- **Modo Host**: escuta conexões, mostra código de sessão, exibe indicador de sessão ativa, botão "Encerrar sessão".
- **Modo Viewer**: campo para IP/porta + código, janela que exibe a tela remota e captura mouse/teclado.

**Por quê:**
- Distribuição trivial (um download, um instalador).
- Todo mundo que instala pode tanto ajudar quanto ser ajudado.
- O código já fica separado internamente em crates (`host`, `viewer`), então extrair binários separados depois é barato.
- Sem serviço de background no MVP: o host é um app comum, aberto por um usuário logado, com janela visível. Isso elimina os problemas de sessão 0/UAC no Windows e de acesso ao display no Linux, e é o comportamento eticamente correto (nada roda escondido).

**Fora do MVP (evolução futura):** serviço Windows / systemd unit para "acesso não assistido", servidor de rendezvous com IDs curtos, relay.

### 2.2 Topologia de conexão do MVP

```
┌─────────────┐        QUIC (TLS 1.3)         ┌─────────────┐
│   VIEWER    │ ─────────────────────────────▶ │    HOST     │
│  (controla) │   LAN direta, IP:porta        │ (controlado) │
└─────────────┘                                └─────────────┘
   stream de controle (bidirecional, confiável):
     handshake, auth, input, resize, ping
   streams de mídia (unidirecionais host→viewer):
     frames de tela
```

- Host abre porta (padrão configurável, ex.: `21118/udp`).
- Viewer conecta com `IP:porta` + código de sessão.
- Sem servidor externo, sem NAT traversal no MVP — só LAN (ou VPN/port-forward por conta do usuário).

### 2.3 Runtime interno

- `tokio` como runtime async para rede e orquestração.
- Captura e encode rodam em threads dedicadas (são trabalho bloqueante/CPU-bound), comunicando com a camada de rede por canais (`tokio::sync::mpsc` / `crossbeam-channel`).
- UI (egui) roda na thread principal com seu próprio loop; troca mensagens com o núcleo por canais. **UI nunca acessa rede/captura diretamente** — isso mantém host/viewer/transporte/codec/UI desacoplados, como exigido.

---

## 3. Stack sugerida (com justificativas)

### 3.1 UI desktop — `egui`/`eframe` ✅

| Opção | Prós | Contras | Veredito |
|---|---|---|---|
| **egui/eframe** | 100% Rust; excelente para pintar textura atualizada a cada frame (caso de uso exato do viewer: `egui::TextureHandle` atualizada por frame); imediato = sem sincronização de estado; maduro, muito usado | Visual menos "nativo"; acessibilidade limitada | **Recomendado** |
| Slint | Bonito, declarativo, bom tooling | DSL própria (`.slint`) — menos "tudo em Rust"; licença GPL/royalty-free com condições | 2º lugar |
| Iced | Elm-like, elegante | Renderização de vídeo/textura streaming mais burocrática; API ainda instável entre versões | Não p/ este caso |
| Tauri | Ecossistema web | UI em HTML/JS — contraria a preferência por Rust; webview pesa para exibir 30fps de tela remota | Descartado |

Para um app cuja tela principal é literalmente "desenhar um bitmap grande 30x por segundo e capturar eventos de mouse/teclado sobre ele", imediato + textura do egui é o encaixe perfeito. A UI de host (código de sessão, botão encerrar) é trivial em qualquer framework.

### 3.2 Rede/transporte — QUIC via `quinn` ✅

| Opção | Análise |
|---|---|
| **QUIC (`quinn`)** | TLS 1.3 obrigatório e embutido (resolve a criptografia do canal "de graça"); múltiplos streams sem head-of-line blocking entre input e vídeo; datagramas não confiáveis disponíveis para frames descartáveis no futuro; roda sobre UDP → **é a mesma base que serve para NAT hole punching na fase de internet** (não joga fora o trabalho). Puro Rust (`quinn` + `rustls`). **Recomendado** |
| TCP + TLS (`tokio-rustls`) | Mais simples conceitualmente; mas input e vídeo no mesmo stream sofrem head-of-line blocking (um frame grande atrasa o clique); dois sockets complicam. Alternativa conservadora aceitável se quiser reduzir dependências |
| WebSocket (`tungstenite`) | Só faz sentido se houvesse viewer browser — não há. Overhead sem ganho |
| WebRTC (`webrtc-rs`) | Resolve NAT traversal e mídia, mas é uma stack enorme e complexa para um MVP LAN; fica como **opção da fase de internet**, competindo com "rendezvous próprio + hole punching QUIC + relay próprio" |
| gRPC | Modelo request/response não serve para streaming de tela em tempo real com input de baixa latência |
| UDP customizado | Reinventar confiabilidade/criptografia = risco de segurança. Descartado |

**Divisão de streams QUIC:**
- 1 stream bidirecional de **controle**: Hello, auth, input, resize, ping, disconnect (confiável e ordenado — input não pode se perder).
- streams unidirecionais host→viewer para **frames** (1 stream por frame ou stream contínuo; a decidir na implementação).

**Fase futura (internet):** servidor de **rendezvous** próprio (registro de IDs curtos + troca de endereços) → tentativa de **UDP hole punching com o mesmo QUIC** → fallback para **relay** próprio (encaminha bytes cifrados, sem poder lê-los, pois o TLS é fim-a-fim entre host e viewer). WebRTC/ICE ficaria como plano B se o hole punching caseiro se mostrar insuficiente; TURN é essencialmente o "relay" com protocolo padronizado.

### 3.3 Captura de tela — `xcap` ✅ (com saída de emergência para APIs nativas)

Consultado via Context7: `xcap` é cross-platform (Windows via GDI + Windows Graphics Capture; Linux X11 e Wayland via PipeWire/libwayshot), retorna `RgbaImage` e tem API `video_recorder()` que entrega frames RGBA crus por canal em thread própria — exatamente o formato que o pipeline precisa.

**Windows:**
- Ideal a longo prazo: **DXGI Desktop Duplication** ou **Windows Graphics Capture** direto via `windows-rs` (dirty rects nativos, zero-copy GPU).
- MVP: `xcap` (que já usa WGC/GDI por baixo). Se o desempenho decepcionar, trocar o backend Windows pela crate `windows-capture` (WGC dedicada) ou Desktop Duplication manual — a interface do nosso crate `capture` isola essa troca.
- FFI direto não é necessário: `windows-rs` é oficial da Microsoft e já é o que essas crates usam.

**Linux X11:**
- `xcap` usa XCB/SHM. Suficiente para MVP.
- Limitação conhecida: sem dirty rects nativos no X11 → detecção de mudança será feita por comparação de tiles no nosso codec (ver 3.6).

**Linux Wayland (fase posterior):**
- Único caminho legítimo: **xdg-desktop-portal (ScreenCast) + PipeWire**, com diálogo de consentimento do usuário mostrado pelo compositor — não existe bypass, e não queremos um.
- Crates: `ashpd` (portais, mantida por dev do GNOME) + `pipewire-rs` (bindings oficiais). O `xcap` também tem suporte Wayland via portal, o que pode encurtar o caminho.
- Consequências que o produto precisa aceitar: diálogo de permissão a cada sessão (mitigável com `persist_mode` do portal), possível impossibilidade de capturar sem interação do usuário — o que para nós é aceitável, pois consentimento explícito é requisito do produto.

### 3.4 Injeção de mouse/teclado — `enigo` ✅

Consultado via Context7:

**Windows — `SendInput` (é o que o enigo usa):**
- Funciona para tudo que roda no nível de integridade do próprio app.
- **Limitação documentada (UIPI):** não injeta em janelas elevadas (admin), no prompt de UAC nem na tela de login, a menos que o próprio host rode elevado. **Não vamos contornar isso** — é o comportamento de segurança correto do Windows. O plano: documentar a limitação; futuramente oferecer "executar host como administrador" mediante ação explícita do usuário (com o UAC normal do sistema).

**Linux X11 — XTest (backend padrão do enigo, via `x11rb`):**
- Sem permissões especiais quando rodando na sessão do usuário. Mapeamento de layout via XKB já tratado pelo enigo.
- `xdotool`/`libxdo`: desnecessário, XTest via x11rb cobre tudo. `evdev`/`uinput`: exige root/regras udev e injeta "abaixo" do layout de teclado (complica mapeamento) — não vale para MVP; só reconsiderar para o cenário de acesso à tela de login, que está fora de escopo.

**Linux Wayland (fase posterior) — portal `RemoteDesktop`:**
- `ashpd` expõe `RemoteDesktop::notify_keyboard_keycode` / eventos de ponteiro, na mesma sessão do portal de ScreenCast (confirmado na doc: a sessão pode combinar os dois). Consentimento do usuário via diálogo do sistema — de novo, alinhado com nossos requisitos éticos.
- `enigo` tem backend Wayland experimental usando exatamente esse mecanismo (e `libei`); quando chegarmos lá, decidir entre enigo-wayland e uso direto do `ashpd` (provável: `ashpd` direto, porque a sessão de captura e input precisam ser a mesma).

**Abstração:** nosso crate `input` define um trait (`InputInjector`) com implementações por plataforma; enigo por baixo no MVP. Coordenadas trafegam **normalizadas (0.0–1.0) por monitor** no protocolo, e o host converte para pixels físicos — isso resolve DPI scaling e resoluções diferentes de forma estrutural.

### 3.5 Teclado — decisão de design importante

Enviar **scancodes/keycodes posicionais + a representação Unicode** quando disponível. Estratégia do MVP: enviar a tecla lógica (enum de teclas nomeadas estilo enigo `Key`) + fallback `Key::Unicode(char)` para caracteres imprimíveis. Layouts diferentes entre viewer e host são um dos riscos clássicos (seção 11); começamos com essa dupla e refinamos com testes reais pt-BR/ABNT2 vs US.

### 3.6 Codec/compressão

| Fase | Técnica | Justificativa |
|---|---|---|
| MVP passo 1 | Raw RGBA (LAN, só para validar o pipeline) | Zero dependência; ~250 MB/s a 1080p30 — só aguenta em loopback/LAN gigabit, e é temporário |
| **MVP final** | **Dirty tiles + JPEG**: frame dividido em blocos (ex.: 64×64), hash/comparação por bloco, só blocos alterados são enviados, comprimidos em JPEG (qualidade ~80) | Simples, 100% implementável em Rust, desempenho aceitável para desktop típico (tela majoritariamente estática). Encoder: `turbojpeg` (bindings libjpeg-turbo — rápido, mas FFI/C) ou `jpeg-encoder`/`zune-jpeg` (Rust puro, mais lento). Recomendo **começar com Rust puro** e trocar por turbojpeg só se medição mostrar necessidade |
| Fase codec | **OpenH264** (crate `openh264`, bindings do encoder da Cisco) ou **VP8/VP9** (crate `vpx`/bindings libvpx) | Vídeo real com estimação de movimento: necessário para vídeo/scroll fluido. Aqui **não recomendo Rust puro** — não existe encoder H.264/VP9 maduro em Rust (rav1e existe para AV1, mas é lento demais para tempo real em software). Trade-off explícito na seção "onde não é 100% Rust" abaixo |
| Futuro | Hardware encoding (NVENC/QuickSync/VAAPI, possivelmente via GStreamer) | Só quando houver demanda real; GStreamer adiciona dependência de sistema pesada |

**PNG**: lento demais para tempo real. **MJPEG**: é essencialmente o que "dirty tiles + JPEG" faz, porém mais inteligente. **AV1**: encode por software não alcança tempo real com CPU comum. **FFmpeg/GStreamer**: poderosos, mas são dependências de sistema grandes e de empacotamento chato; adiar até a fase de hardware encoding.

### 3.7 Onde NÃO recomendo 100% Rust (transparência exigida)

1. **Encoder de vídeo (fase codec, pós-MVP):** não há encoder H.264/VP8/VP9 tempo-real maduro em Rust puro. Alternativas: `openh264` (binding fino, licença BSD, binário da Cisco cobre royalties H.264) ou libvpx. Trade-off: FFI e binário C embutido vs. qualidade/latência muito superiores a JPEG. **Recomendação: openh264 quando chegar a hora; o MVP não precisa disso.**
2. **libjpeg-turbo (opcional):** só se o JPEG Rust puro não der conta; medir antes.
3. **PipeWire (fase Wayland):** `pipewire-rs` são bindings sobre a lib C do sistema. Inevitável e aceitável — é a API oficial do sistema, como `windows-rs` no Windows.

Todo o resto — rede, TLS, protocolo, UI, captura, input, storage — fica em Rust.

### 3.8 Banco de dados — SQLite local (`rusqlite`, feature `bundled`)

O MVP **quase** não precisa de banco. Decisão:

- **Configurações** → arquivo TOML em diretório padrão da plataforma (crate `directories`), legível e editável pelo usuário.
- **SQLite local** para dados que crescem/consultam:
  - logs de conexão (quem, quando, IP, resultado auth, duração) — requisito de segurança;
  - dispositivos conhecidos (fingerprint do certificado do peer + apelido) — base do TOFU;
  - futuros tokens/pareamentos.
- **Nunca** armazenar senha/código em texto puro: código de sessão vive só em memória; se um dia houver senha permanente, hash com `argon2`.
- PostgreSQL: só se/quando existir o servidor de rendezvous (fase internet), e será decisão daquele momento.

### 3.9 Crates principais propostos (resumo)

| Crate | Papel | Observação |
|---|---|---|
| `tokio` | runtime async | padrão de fato |
| `quinn` + `rustls` + `rcgen` | QUIC + TLS + geração de cert | rcgen gera o cert self-signed persistente do host |
| `serde` + `postcard` | serialização do protocolo | ver seção 6 |
| `eframe`/`egui` | UI | |
| `xcap` | captura | isolado atrás de trait |
| `enigo` | injeção de input | isolado atrás de trait |
| `ashpd`, `pipewire` | Wayland (fase futura) | |
| `rusqlite` | storage | `bundled` para não depender de sqlite do sistema |
| `argon2`, `rand`, `subtle` | segurança | hash, geração de código, comparação constant-time |
| `tracing` + `tracing-subscriber` | logs | |
| `directories`, `toml` | config | |
| `thiserror`/`anyhow` | erros | thiserror nas libs, anyhow no app |
| `jpeg-encoder`/`zune-jpeg` (ou `turbojpeg`) | codec MVP | decidir por medição |

---

## 4. Escopo do MVP e roadmap por fases

**Definição do MVP:** viewer em uma máquina vê a tela do host em outra máquina na mesma LAN e controla mouse/teclado, com autenticação por código, canal cifrado, indicador visível no host e botão de encerrar — em Windows e Linux X11.

Reordenei as fases sugeridas no pedido: como as crates escolhidas são cross-platform, **Windows e Linux X11 são desenvolvidos juntos** (fase de plataforma vira fase de estabilização), e autenticação entra cedo porque o protocolo nasce com ela (retrofit de segurança é sempre pior).

### Fase 0 — Fundação do workspace
- **Objetivo:** monorepo, CI, crates vazios com traits definidos.
- **Entregável:** `cargo build && cargo test && cargo clippy` verdes em Linux e Windows (CI).
- **Aceite:** estrutura da seção 5 compilando; lints (`clippy`, `rustfmt`) configurados.
- **Riscos:** nenhum relevante.
- **Teste:** CI.

### Fase 1 — Protocolo + transporte LAN
- **Objetivo:** conexão QUIC viewer→host com handshake Hello/versão e Ping/Pong.
- **Entregável:** dois processos na mesma máquina/LAN trocando mensagens tipadas.
- **Aceite:** handshake completa; versão incompatível é rejeitada com `Error`; reconexão manual funciona; testes de roundtrip de serialização passam.
- **Riscos:** ergonomia do quinn com certs self-signed → mitigado: fingerprint TOFU já nesta fase.
- **Teste:** testes de integração in-process (endpoint client+server no mesmo teste tokio).

### Fase 2 — Autenticação e autorização
- **Objetivo:** código de sessão temporário + aprovação manual no host + rate limiting.
- **Entregável:** host exibe código de 8 caracteres; viewer só entra com código correto; host mostra popup "Fulano (IP x.x.x.x) quer conectar — Aceitar/Recusar".
- **Aceite:** código errado 5× → bloqueio temporário do IP com backoff; código expira ao ser usado ou por timeout; tudo logado no SQLite; comparação de código em tempo constante.
- **Riscos:** UX de aprovação; brute force → backoff exponencial.
- **Teste:** unitários de rate limit/expiração; integração com código errado/correto/expirado.

### Fase 3 — Captura e streaming de tela
- **Objetivo:** host captura monitor primário e envia; viewer exibe em janela egui.
- **Entregável:** vídeo da tela remota visível no viewer, primeiro raw, depois dirty tiles + JPEG.
- **Aceite:** ≥15 fps a 1080p em LAN com tela em atividade normal; latência visual < 200 ms na LAN; banda em tela parada próxima de zero (graças aos dirty tiles); redimensionar a janela do viewer mantém proporção.
- **Riscos:** desempenho de captura X11; throughput JPEG puro-Rust → medir e, se preciso, turbojpeg.
- **Teste:** benchmark de encode; teste de tiles (frame alterado em 1 bloco → só 1 bloco enviado); teste visual manual com checklist.

### Fase 4 — Controle de mouse e teclado
- **Objetivo:** eventos do viewer aplicados no host.
- **Entregável:** controle completo: mover, clicar (3 botões), scroll, digitar (incl. acentos pt-BR), atalhos (Ctrl+C etc.).
- **Aceite:** clique acerta o pixel esperado com host e viewer em resoluções/DPI diferentes (coordenadas normalizadas); digitação de "ação já çê" chega correta; modificadores não "vazam" (soltar Ctrl ao desconectar).
- **Riscos:** layouts de teclado; foco/captura de atalhos pelo SO do viewer.
- **Teste:** property tests de conversão de coordenadas; matriz manual de teclado US/ABNT2; teste de "modificador preso".

### Fase 5 — Estabilização Windows + Linux X11 (o "MVP completo")
- **Objetivo:** as 4 combinações host/viewer × Windows/Linux funcionando.
- **Entregável:** MVP utilizável ponta a ponta, com indicador de sessão ativa, botão encerrar, logs, tratamento de desconexão (viewer avisa e permite reconectar; host libera input e volta a exibir código novo).
- **Aceite:** checklist manual completo nas 4 combinações; queda de rede não deixa host em estado inconsistente; nenhuma tecla presa após desconexão abrupta.
- **Riscos:** diferenças de DPI por monitor no Windows; UIPI (janelas elevadas) — documentar.
- **Teste:** matriz manual formalizada em `docs/test-matrix.md` + testes automatizados de reconexão em loopback.

### Fase 6 — Multi-monitor, DPI e desempenho
- **Objetivo:** seleção de monitor no viewer (`MonitorList`/`SelectMonitor`), DPI correto, otimizações medidas.
- **Aceite:** troca de monitor ao vivo; coordenadas corretas em setup multi-monitor com scaling misto (ex.: 100% + 150%).

### Fase 7 — Codec de vídeo real
- **Objetivo:** OpenH264 (ou VP8) atrás do trait `Encoder`, negociado no handshake (host e viewer anunciam codecs; fallback para JPEG-tiles).
- **Aceite:** 30 fps a 1080p com ~5–15% CPU de encode e banda < 4 Mbps em uso típico; vídeo do YouTube na tela remota assistível.

### Fase 8 — Rendezvous + relay (internet)
- **Objetivo:** servidor Rust (`server/rendezvous`) com IDs curtos, hole punching UDP/QUIC, fallback relay cifrado fim-a-fim.
- **Aceite:** conexão entre duas redes domésticas distintas sem port-forward; relay não consegue ler o tráfego.

### Fase 9 — Wayland (host Linux moderno)
- **Objetivo:** captura via portal ScreenCast + PipeWire e input via portal RemoteDesktop (`ashpd`), na mesma sessão de portal.
- **Aceite:** GNOME e KDE (wlroots como best-effort); diálogo de consentimento do sistema respeitado; `persist_mode` para reduzir re-prompts.

*(Empacotamento — seção 9 — entra em paralelo a partir da fase 5.)*

---

## 5. Estrutura do monorepo

```
controlis/
├── Cargo.toml                # workspace
├── crates/
│   ├── protocol/             # mensagens, serialização, versionamento, framing.
│   │                         #   Zero deps de SO. Compila até em CI mínimo.
│   ├── transport/            # QUIC (quinn): endpoint, streams tipados,
│   │                         #   certs (rcgen), fingerprint TOFU, reconexão.
│   ├── capture/              # trait ScreenCapturer + backends (xcap; depois
│   │                         #   pipewire). Enumeração de monitores.
│   ├── input/                # trait InputInjector + backends (enigo; depois
│   │                         #   portal). Conversão de coordenadas normalizadas.
│   ├── codec/                # trait Encoder/Decoder: raw, dirty-tiles+JPEG;
│   │                         #   depois openh264. Negociação de codec.
│   ├── security/             # código de sessão, rate limiting, argon2,
│   │                         #   comparação constant-time, fingerprints.
│   ├── storage/              # config TOML + SQLite (logs, peers conhecidos).
│   ├── session-host/         # orquestração do lado host: aceitar conexão,
│   │                         #   auth, loop captura→encode→envio, aplicar input.
│   └── session-viewer/       # orquestração do lado viewer: conectar, auth,
│                             #   receber/decodificar frames, enviar input.
├── apps/
│   └── controlis/            # binário único: UI egui, telas host/viewer,
│                             #   indicador de sessão, wiring dos crates.
├── server/                   # (vazio até a fase 8) rendezvous/relay.
├── docs/                     # este plano, protocolo, matriz de testes.
└── xtask/                    # automação (dist, lint) se necessário.
```

Regras de dependência (aplicadas via `cargo` mesmo): `protocol` não depende de nada interno; `session-*` dependem dos crates de capacidade; **só `apps/controlis` conhece egui**; `capture`/`input` nunca conhecem rede. Evitei crate `core`/`utils` genérico de propósito — cada crate tem responsabilidade nomeada.

---

## 6. Protocolo interno

### 6.1 Formato: `serde` + `postcard`, length-delimited

| Opção | Avaliação |
|---|---|
| **serde + postcard** | Binário compacto (varint), Rust puro, sem schema externo, ótimo para enums Rust. **Recomendado para MVP** |
| serde + bincode | Equivalente; postcard é mais compacto e tem semântica de formato mais estável |
| protobuf (prost) | Melhor evolução de schema entre versões — **candidato natural quando houver servidor/relay público e clientes de versões mistas**; para MVP é cerimônia extra |
| capnp/flatbuffers | Zero-copy interessante para frames, mas os frames grandes já viajam como bytes opacos fora do serde (ver abaixo) — ganho não justifica |
| JSON | Apenas atrás de flag de debug/log, nunca no wire por padrão |

**Versionamento desde o dia 1:** `Hello { protocol_version: u16, ... }` e rejeição explícita de versão incompatível. Payload de frame (`Vec<u8>` já codificado pelo codec) trafega com `serde_bytes` para evitar overhead de serialização elemento a elemento.

**Framing:** cada mensagem prefixada por tamanho (u32 LE) dentro do stream QUIC; frames de tela em streams unidirecionais separados para não bloquear o stream de controle.

### 6.2 Mensagens (v1)

```
// Controle (stream bidirecional confiável)
Hello            { protocol_version, app_version, role, supported_codecs }
AuthRequest      { session_code }                       // viewer → host
AuthChallenge    { }                                    // reservado p/ evolução (SPAKE2)
AuthResponse     { Accepted { session_id } | Rejected { reason } | PendingApproval }
MonitorList      { monitors: [ { id, name, width_px, height_px, is_primary } ] }
SelectMonitor    { monitor_id }
MouseMove        { x_norm: f32, y_norm: f32 }           // normalizado 0..1 no monitor ativo
MouseButton      { button: Left|Right|Middle, action: Press|Release }
MouseWheel       { delta_x: f32, delta_y: f32 }
KeyEvent         { key: KeyCode, action: Press|Release }  // KeyCode = enum nomeado + Unicode(char)
Resize           { }                                     // host avisa mudança de resolução
Ping { nonce } / Pong { nonce }
Disconnect       { reason }
Error            { code, message }

// Mídia (streams unidirecionais host → viewer)
ScreenFrame      { monitor_id, frame_id, codec, encoding: Full|TileDelta,
                   tiles: [ { x, y, w, h } ], payload: bytes }
```

`KeyDown`/`KeyUp` do pedido viraram `KeyEvent { action }` (mesma expressividade, menos duplicação); idem mouse. O documento `docs/protocol.md` será a fonte de verdade e evolui com o código.

---

## 7. Segurança mínima do MVP (inegociável)

1. **Canal:** QUIC = TLS 1.3 sempre; impossível conexão em claro.
2. **Identidade do host:** certificado self-signed **persistente** (gerado com `rcgen` no primeiro uso). Viewer faz **TOFU**: primeiro contato exibe fingerprint (e o host mostra o dele para conferência verbal); contatos seguintes exigem o mesmo cert — mudança = alerta forte. Fingerprints ficam no SQLite.
3. **Autenticação:** código de sessão aleatório (CSPRNG, ~8 chars, alfabeto sem ambiguidade tipo `0/O`), exibido no host, de uso único e com expiração (ex.: 10 min). Comparação constant-time (`subtle`). Nunca persistido em claro.
4. **Autorização humana:** mesmo com código correto, host exibe prompt Aceitar/Recusar (com opção de configuração "aceitar automaticamente com código" — padrão: perguntar).
5. **Anti brute-force:** contador por IP; backoff exponencial; N falhas → bloqueio temporário; tudo logado.
6. **Sessão visível:** indicador permanente e inequívoco no host durante a sessão (janela/ícone com destaque) + **botão "Encerrar sessão" sempre acessível** + novo código gerado após cada sessão.
7. **Logs locais:** SQLite: timestamp, IP, fingerprint do viewer, sucesso/falha, duração, motivo do término.
8. **Higiene de input:** ao desconectar (inclusive queda), host solta todas as teclas/botões pressionados (proteção contra "Ctrl preso").
9. **O que não faremos:** nenhum bypass de UAC/permissões/consentimento Wayland; nenhum modo oculto; nenhuma persistência furtiva.
10. **Evolução (pós-MVP):** SPAKE2/OPAQUE para transformar o código de sessão em autenticação mútua resistente a MITM ativo mesmo sem TOFU prévio (a mensagem `AuthChallenge` já reserva o espaço no protocolo).

---

## 8. Riscos técnicos e mitigações

| Risco | Impacto | Mitigação |
|---|---|---|
| Desempenho de captura/encode insuficiente | MVP inutilizável | Dirty tiles desde o MVP; benchmarks como critério de aceite da fase 3; plano B: turbojpeg → windows-capture/DXGI → fase 7 (H.264) |
| Latência de input perceptível | UX ruim | Stream de controle separado dos frames (sem head-of-line blocking no QUIC); input tem prioridade |
| Wayland (consentimento, variação entre compositores) | Host Linux moderno limitado | X11 primeiro (declarado ao usuário); fase 9 via portais oficiais; testar GNOME/KDE; `persist_mode` p/ reduzir prompts. XWayland não resolve host Wayland — não fingir que resolve |
| UAC/janelas elevadas no Windows | Viewer "perde controle" em prompts admin | Documentar; detectar e avisar o viewer quando possível; opção futura de rodar host elevado (via UAC normal) |
| Antivírus/SmartScreen | Usuário não consegue instalar | Assinatura de código (certificado OV/EV — custo a aprovar); binários reproduzíveis; nunca técnicas "stealth" que aumentam heurística de detecção |
| NAT/firewall (fase 8) | Conexão internet falha | Hole punching + relay fallback; relay cifrado fim-a-fim; documentar porta do host p/ LAN |
| Custo de banda do relay | Custo operacional futuro | Relay só como fallback; codec eficiente antes da fase 8; limites de taxa no relay |
| Múltiplos monitores + DPI misto | Cliques errados | Coordenadas normalizadas por monitor desde a v1 do protocolo; fase 6 dedicada; property tests |
| Layouts de teclado divergentes | Digitação errada | Estratégia dupla (tecla nomeada + Unicode); matriz de teste US/ABNT2; refinamento iterativo |
| Crates de captura/input imaturos em edge cases | Bugs de plataforma | Traits isolam backends; trocar xcap/enigo por implementação nativa direta é local, não sistêmico |
| Manutenção multiplataforma | Regressões cruzadas | CI em Windows+Linux; matriz de teste manual formal antes de cada release |
| Segurança do protocolo | Acesso indevido | Seção 7 inteira; revisão de segurança dedicada antes de qualquer release público |

---

## 9. Empacotamento e distribuição

**Windows:**
- MVP: instalador **MSI via `cargo-wix`** (ou NSIS se precisarmos de mais customização) + zip portátil.
- Sem serviço Windows no MVP (app de usuário). Serviço só na fase de acesso não assistido, instalado explicitamente com consentimento.
- SmartScreen: risco real para binário não assinado → decidir sobre certificado de assinatura (custo anual).

**Linux:**
- MVP: **.deb via `cargo-deb`** + **AppImage** (cobre a maioria das distros sem exigir empacotamento por distro). `.rpm` via `cargo-generate-rpm` quando houver demanda.
- systemd service: só na fase de acesso não assistido.
- **Flatpak: não recomendado para o host**, e vale explicar: o sandbox do Flatpak bloqueia exatamente o que um host de acesso remoto precisa — captura de tela fora dos portais, XTest/uinput para injeção de input, e acesso amplo ao X11. Via Wayland + portais (ScreenCast/RemoteDesktop) um host flatpakado é *teoricamente* viável (é como o RustDesk Flatpak funciona, com limitações), mas no X11 exigiria furar o sandbox (`--socket=x11` + permissões amplas), o que anula o propósito do Flatpak e gera atrito de revisão no Flathub. Reavaliar somente na fase 9, para o cenário Wayland-only. Para o **viewer**, Flatpak seria tranquilo — mas como é binário único, fica adiado junto.

---

## 10. Plano de testes

| Camada | Estratégia |
|---|---|
| `protocol` | Roundtrip de serialização de toda mensagem; rejeição de versão incompatível; fuzzing leve de decodificação (entrada malformada nunca causa panic) |
| `security` | Unitários: expiração de código, uso único, rate limit/backoff, comparação constant-time (por construção), hash argon2 |
| `transport` | Integração in-process (client+server tokio no mesmo teste): handshake, TOFU (cert trocado → falha), desconexão abrupta, reconexão |
| `codec` | Tile alterado → só ele transmitido; encode+decode = imagem equivalente (tolerância JPEG); benchmarks com `criterion` |
| Coordenadas/DPI | Property tests (`proptest`): normalização↔pixel é consistente para qualquer resolução/scaling; multi-monitor com offsets |
| `capture`/`input` | Smoke tests marcados `#[ignore]` em CI headless; rodados localmente e em VMs com desktop; no X11 do CI, `Xvfb` permite testar captura+XTest de verdade (injetar tecla → capturar efeito) |
| Reconexão/perda | Teste de integração matando a conexão no meio do stream: host libera input (nenhuma tecla presa) e volta ao estado "aguardando"; viewer mostra erro e permite reconectar |
| Ponta a ponta | Matriz manual formal (`docs/test-matrix.md`): {Win host, Linux X11 host} × {Win viewer, Linux viewer} × checklist (auth ok/errada, digitação pt-BR, atalhos, scroll, resize, multi-monitor, encerrar pelo host, queda de rede). Wayland entra na matriz na fase 9 (GNOME e KDE) |

CI: GitHub Actions com jobs Linux e Windows (`build + clippy -D warnings + fmt --check + test`), mais job Linux com Xvfb para os testes de captura/input X11.

---

## 11. Onde usei Context7/pesquisa neste plano

- **`enigo`** (/enigo-rs/enigo): confirmado — Windows/X11 estáveis por padrão (SendInput/XTest via x11rb, XKB para layout); Wayland/libei **experimental atrás de feature flags**, usando o protocolo de remote desktop via D-Bus. Baseou as seções 3.4 e a decisão de adiar Wayland.
- **`ashpd`** (/bilelmoussaoui/ashpd): confirmado — portal RemoteDesktop injeta teclado/ponteiro e **combina com ScreenCast na mesma sessão**, com PipeWire node IDs para o pipeline de mídia e `persist_mode` para reduzir prompts. Baseou a fase 9.
- **`xcap`** (/nashaofu/xcap): confirmado — Windows (GDI + Windows Graphics Capture), Linux (X11 e Wayland via PipeWire/libwayshot), API `video_recorder()` entregando frames RGBA crus por canal; video recording marcado como "em desenvolvimento" no README → por isso o plano B (windows-capture/DXGI) e o trait isolando o backend. Baseou a seção 3.3.
- **`quinn`** (/quinn-rs/quinn): confirmado — server/client com rustls, certs self-signed via rcgen, streams bi/unidirecionais com tokio, e verificação de certificado customizável (necessária para o modelo TOFU). Baseou as seções 3.2 e 7.
- egui/eframe verificado no índice (alta cobertura de docs); consultas mais profundas (texture streaming, integração winit) ficam para a fase de implementação, como manda a regra de consultar antes de adicionar cada crate.

---

## 12. Decisões pendentes de aprovação

1. **Nome temporário:** "Controlis" (nome da pasta do projeto). Ok?
2. **UI:** egui/eframe. Ok?
3. **Modelo de app:** binário único com modos host/viewer. Ok?
4. **Conexão inicial:** LAN direta (IP:porta + código), sem servidor. Ok?
5. **Transporte:** QUIC (`quinn`) — ou prefere a alternativa conservadora TCP+TLS?
6. **Codec do MVP:** dirty tiles + JPEG (Rust puro primeiro, turbojpeg se medição exigir). Ok?
7. **Ordem de plataforma:** desenvolver cross-platform com **Linux X11 como piloto** (sua máquina de dev) e Windows validado em cada fase — ou prefere Windows como piloto?
8. **Banco:** TOML (config) + SQLite/`rusqlite` (logs, peers). Ok?
9. **Empacotamento:** MSI (cargo-wix) + .deb + AppImage; Flatpak adiado. Ok?
10. **Estrutura do monorepo:** conforme seção 5. Ok?
11. **Crates principais:** conforme seção 3.9. Ok?
12. **FFI aceito em princípio para fase futura de codec** (openh264/libvpx) e, se necessário por desempenho, turbojpeg no MVP — precisa da sua anuência explícita por ser exceção ao "100% Rust".
13. **Teclado:** estratégia "tecla nomeada + Unicode" no MVP (aceitando refinamento iterativo para atalhos exóticos). Ok?
14. **Segurança:** modelo TOFU + código de sessão + aprovação manual (SPAKE2 pós-MVP). Ok?
