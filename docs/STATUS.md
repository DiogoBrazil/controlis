# STATUS do Controlis — diário de evolução e pendências

> Última atualização: **2026-07-09** (smoke loopback da UI Tauri VALIDADO com
> teardown limpo; logging do viewer implementado; templates de deploy do VPS
> prontos em `deploy/`; plano de testes aprovado — ver seção 5).
> Este arquivo é o ponto de retomada: o que está pronto, o que foi observado
> nos testes e o que falta revisar. Complementa o `PLANO-TECNICO.md` (plano) e
> o `TESTE-DUAS-MAQUINAS.md` (roteiro de teste).

## 1. Estado geral das fases

| Fase | Descrição | Estado |
|------|-----------|--------|
| 0–5  | Protocolo, transporte QUIC, captura, input, codec tiles+JPEG, sessões host/viewer, UI egui, storage | ✅ Prontas, 39 testes verdes, clippy limpo |
| 6    | Multi-monitor (origin_x/origin_y, troca ao vivo, seletor no viewer) | ✅ Pronta (e2e cobre troca de monitor) |
| 7    | H.264 via openh264 (feature `h264`, default; negociação no handshake, fallback JpegTiles; PROTOCOL_VERSION=2) | ✅ Implementada. ⚠️ Falta medir banda real (aceite: 30fps@1080p < 4 Mbps) |
| 8    | Rendezvous / relay / NAT traversal (acesso fora da LAN) | ✅ Implementada para primeiro teste real: rendezvous HTTP + Iroh + relay self-host configurado por TOML |
| 9    | Wayland nativo (portal ScreenCast+RemoteDesktop, PipeWire, input via libei/EIS) | ✅ Pronta e validada em runtime no GNOME 46 |

## 2. Cronologia dos testes (2026-07-07)

### 2.1 Validação da Fase 9 na máquina Linux (GNOME 46 / Zorin, Wayland)
- Probe do portal: captura PipeWire 1920x1080 OK.
- GNOME 45+ não honra NotifyPointer*/NotifyKeyboard* do portal → input vai por
  **libei/EIS** (`wayland-portal/src/backend/eis_input.rs`). Ponteiro e teclado
  confirmados funcionando (cursor mexeu; texto digitado).

### 2.2 Incidente: congelamento do gnome-shell (~17:10)
- Telas travaram logo após abrir uma sessão de portal; foi preciso reboot
  forçado. O journal mostra que **o kernel continuou vivo** (cron rodou às
  17:15) — foi o compositor (mutter/gnome-shell) que congelou.
- Suspeitos: stream PipeWire estagnado (host sem consumir buffers) ou cliente
  EIS em estado ruim no encerramento.
- **Mitigação se repetir:** `Ctrl+Alt+F3` → TTY → `sudo systemctl restart gdm`
  (não precisa de reboot; preserva logs).
- **Pendência relacionada:** endurecer o teardown da sessão portal/EIS
  (drop/disconnect limpo inclusive em panic/Ctrl+C).
- Não voltou a ocorrer nos testes seguintes do mesmo dia.

### 2.3 Teste na mesma máquina (host + viewer no mesmo desktop)
- Delay do mouse (host→viewer, via captura): **muito bom**.
- "Mouse do viewer não movia o host": **não era bug** — na mesma máquina há um
  único cursor físico; a injeção briga com o movimento real da mão. Conclusão:
  teste de input só é válido com duas máquinas.

### 2.4 Teste com DUAS máquinas (LAN real) ✅ marco importante
Build no Windows funcionou de primeira:
`cargo build --release -p controlis --features real-capture` (toolchain MSVC).

| Cenário | Vídeo | Mouse | Teclado |
|---------|-------|-------|---------|
| **Linux host + Windows viewer** | ✅ | ✅ | ✅ tudo funcionou |
| **Windows host + Linux viewer** | ✅ | ✅ | ⚠️ **BUG: só números e letras maiúsculas** |

## 2.5 Sessão de 2026-07-09 — smoke loopback da UI Tauri + preparação dos testes

### 2.5.1 ✅ Smoke loopback da UI Tauri (item 3.8) — VALIDADO
- Duas instâncias na mesma máquina, isoladas por `XDG_CONFIG_HOME`/`XDG_DATA_HOME`
  (o `AppPaths` usa `directories::ProjectDirs`, que respeita essas variáveis —
  técnica útil para testar host+viewer localmente sem brigar pelo SQLite).
- Fluxo completo OK: código de acesso → aprovação → vídeo H.264 no canvas
  (1920x1080) → TOFU fixado → desconexão.
- **Teardown limpo confirmado 2x** (exit code 0 nas duas instâncias, nenhum
  processo restante, journal sem erros de mutter/gnome-shell/pipewire).
  O freeze do gnome-shell (2.2) NÃO reapareceu → pendência 3.3 validada
  neste cenário (falta confirmar no teste longo entre máquinas).
- ⚠️ Observado: encoder entrega **~19–20 fps** parado e ~16–18 fps com
  movimento (0.5–0.7 Mbps — banda muito abaixo do teto de 4 Mbps). O alvo da
  Fase 7 é 30 fps; investigar (suspeitos: pacing da captura PipeWire ou o
  FRAME_POLL de 15 ms do pump). Não bloqueia os testes.

### 2.5.2 ✅ Etapa 0 do plano — logging do viewer (commit 477968f)
- O viewer não emitia NENHUM log. Agora loga a jornada completa: alvo
  resolvido (LAN/internet) → transporte estabelecido (QUIC com fingerprint /
  Iroh com endpoint) → aguardando aprovação → sessão aceita → monitores →
  TOFU fixado → primeiro frame (com resolução) → desconexão/erros.
- **Causa do silêncio encontrada:** o filtro default era `controlis=info,warn`,
  mas o lib crate do Tauri se chama `controlis_lib` — o filtro nunca casou com
  o backend. Corrigido: default agora cobre `controlis_lib`, `session_viewer`,
  `session_host`, `transport`, `rendezvous` e `wayland_portal` em info, sem
  precisar de `RUST_LOG`.
- Validado em loopback SEM `RUST_LOG` no ambiente.

### 2.5.3 ✅ Templates de deploy do VPS (commit ddf2bae, pasta `deploy/`)
- `deploy/README.md` — passo a passo completo do VPS; `deploy/iroh-relay.toml`
  (schema conferido contra o fonte do iroh-relay **v1.0.2**, a mesma família do
  cliente); units systemd para os dois serviços.
- **Decisões de arquitetura registradas:**
  - O `iroh-relay` 1.x NÃO tem mais STUN — descoberta de endereço é via QUIC
    na porta **7824/udp** (`enable_quic_addr_discovery`).
  - O relay é dono das portas **80/443** (ACME/Let's Encrypt embutido:
    `cert_mode = "LetsEncrypt"`). Por isso NÃO há Caddy/proxy na frente.
  - O rendezvous (`controlis-server`) fica em `http://...:8080` sem TLS neste
    primeiro deploy (o cliente aceita `http://`; a identidade do host é
    protegida pelo pinning TOFU). TLS no rendezvous = hardening futuro.
  - Portas do firewall do VPS: 80/tcp, 443/tcp, 7824/udp, 8080/tcp.
- Requisito do usuário confirmado: há VPS **com domínio** disponível.

## 3. Bugs e pendências (estado em 2026-07-08)

### 3.1 ✅ Teclado no host Windows — CORRIGIDO E VALIDADO (2026-07-08)
- Causa-raiz confirmada no fonte do enigo 0.6.1: no Windows, `Key::Unicode(c)`
  usa `VkKeyScanExW`, que retorna o VK no byte baixo e **flags de shift no byte
  alto**; o enigo montava `VIRTUAL_KEY(vk as u16)` sem mascarar nem aplicar o
  shift → caracteres que exigem shift saíam com case errado/VK inválido.
- Correção (PROTOCOL_VERSION 2→3): digitação agora viaja na nova mensagem
  `ControlMessage::Text { text }`; o host injeta via `InputInjector::text()`.
  No enigo isso usa `Enigo::text()` (SendInput + KEYEVENTF_UNICODE = caractere
  exato, independente de layout/shift — deve resolver acentos/ç também). No
  backend EIS (Linux) a impl default mantém o comportamento que já funcionava.
  `KeyEvent` ficou só para teclas nomeadas e atalhos (Ctrl+C etc.).
- ✅ Validado pelo usuário em 2026-07-08: digitação funcionando corretamente.

### 3.2 ✅ Medição de banda do H.264 — IMPLEMENTADA (medir no teste real)
- O host agora loga a cada ~5 s, no Registro da UI e no tracing:
  `mídia (H264): X.XX Mbps, Y.Y fps`. Aceite da Fase 7: 30fps @ 1080p < 4 Mbps.

### 3.3 ✅ Teardown da sessão portal/EIS — ENDURECIDO (validar no GNOME)
- Cliente EIS: em todo caminho de saída (shutdown, canal fechado, erro do loop)
  agora faz `stop_emulating` nos devices, `connection.disconnect()` e flush.
- Portal: `PortalHandle::drop` aguarda (até 2 s) o ack de `session.close()`
  antes de deixar o processo morrer — sem sessão pendurada no compositor.
- Ctrl+C: tratado via `tokio::signal` → fecha a janela graciosamente → o drop
  chain (sessões → portal → EIS) roda por inteiro. Panics já faziam unwind.
- ✅ **Validado em 2026-07-09** no smoke loopback (2 ciclos completos de sessão
  portal + encerramento, sem freeze, journal limpo — ver 2.5.1). Resta observar
  em sessões longas entre máquinas.

### 3.4 Limitações conhecidas do teclado EIS (Linux host)
- AltGr, dead-keys e acentos compostos ainda não suportados
  (`eis_input.rs`, mapa keysym→keycode). Não bloqueou o teste, mas revisar.
  (Obs.: com a mensagem `Text`, acentos digitados no viewer chegam como
  caracteres prontos — o gap real fica restrito a composição local exótica.)

### 3.5 ✅ Fase 8 — Rendezvous / Relay / NAT
- Fluxo internet implementado para teste: quando `config.toml` contém
  `rendezvous_url` e `relay_url`, o host registra um endpoint Iroh no
  `controlis-server`, renova o TTL, exibe código v1 com ID de rendezvous e
  aceita conexão por Iroh ou LAN.
- O viewer parseia v1, consulta `/v1/sessions/{id}`, conecta via Iroh usando
  relay self-host obrigatório e autentica com o mesmo `AuthRequest.session_code`.
  Se o campo avançado `IP:porta` for preenchido, v1 também pode ser usado como
  fallback LAN manual.
- Pinning: LAN continua por fingerprint TLS (`lan:<ip:porta>`); internet usa
  `peer_key = rendezvous:<id>` e identidade persistida `iroh:<endpoint_id>`.
  Mudança de endpoint conhecido bloqueia a conexão.
- O relay público do Iroh não é usado por padrão; o endpoint é criado com
  `RelayMode::Custom` a partir de `relay_url`.

### 3.5.1 ✅ Preparação pré-Fase 8 — TOFU persistente no app Tauri
- O backend Tauri do viewer agora usa o SQLite para fixar a identidade do host
  por alvo (`peer_key`, hoje `lan:<ip:porta>`). A primeira conexão registra o
  fingerprint apresentado; reconexões passam esse fingerprint esperado ao
  verificador TOFU do transporte.
- Se o certificado de um host conhecido mudar, a conexão é bloqueada com erro
  explícito de identidade alterada. Não há UI de "esquecer host" ainda; se o
  host for reinstalado, o reset do pin continua manual.
- O storage ganhou `known_host`, separado de `known_peer`, para que a Fase 8
  reutilize o mesmo modelo com chaves como `rendezvous:<id>` sem mudar o
  mecanismo de pinning.

### 3.5.2 ✅ Fase 8 — rendezvous HTTP + transporte Iroh
- `crates/security::ConnectCode` agora suporta `ConnectTarget::Lan` (v0,
  compatível) e `ConnectTarget::Rendezvous { id }` (v1). O código continua com
  16 caracteres Crockford no formato `XXXX-XXXX-XXXX-XXXX`.
- `crates/rendezvous` define os DTOs compartilhados e as regras do store em
  memória: TTL máximo de 120 s, token de posse por registro, colisão bloqueada
  enquanto o registro está ativo e reuso permitido após expiração.
- `servers/controlis-server` é o primeiro processo para VPS/self-host: sem
  persistência em disco por enquanto, configurável por
  `CONTROLIS_SERVER_BIND` (default `0.0.0.0:8080`).
- `crates/transport` agora encapsula Quinn (LAN) e Iroh (internet) sob a mesma
  API de sessão. A chave Iroh é persistida em `iroh_secret_key.txt` no diretório
  de dados do app.
- `config.toml` ganhou `rendezvous_url` e `relay_url`; se faltarem ou se Iroh
  não ficar online em ~8 s, o host registra no log e volta para código LAN.

### 3.6 Melhorias de UX/robustez
- ✅ A UI do host agora mostra o backend ativo ("Backend: Wayland portal
  (PipeWire + EIS)" / "xcap + enigo"); fallback do portal aparece no Registro.
- ⬜ Documentar/automatizar regra de firewall no Windows host (UDP). Comando a
  testar na Etapa 1 (PowerShell admin): `netsh advfirewall firewall add rule
  name="Controlis" dir=in action=allow protocol=UDP localport=21118`.
- ⬜ **fps abaixo do alvo:** encoder a ~19–20 fps (alvo 30) no smoke loopback a
  1080p (ver 2.5.1). Confirmar se persiste entre máquinas e investigar
  (pacing PipeWire? `FRAME_POLL` 15 ms do pump do viewer?).
- ⬜ TLS no rendezvous (hoje `http://:8080`; ver decisão em 2.5.3).

### 3.8 ✅ UI migrada para Tauri v2 + Leptos — 2026-07-08 (validar em 2 máquinas)
- O app egui foi substituído por `apps/controlis-app`: frontend Leptos (CSR,
  Trunk, tema escuro responsivo no padrão do host-deck) + backend Tauri
  (`src-tauri`) reusando os crates de sessão sem mudanças de protocolo.
- Vídeo no viewer: quadro decodificado → JPEG (`codec::encode_rgba_to_jpeg`)
  → `tauri::ipc::Channel` binário → `createImageBitmap` → canvas (letterbox).
- Input: `public/js/screen.js` captura pointer/teclado no canvas; digitação
  vira `Text`, atalhos/teclas nomeadas viram `KeyEvent` (sem diff de
  modificadores). `blur` solta modificadores para não prender tecla no host.
- Dev: `cd apps/controlis-app/src-tauri && cargo tauri dev` (features
  `real-capture`/`wayland` via `--features`). Smoke: app abre, Ctrl+C encerra
  limpo (mesmo caminho de teardown das sessões/portal).
- Tela do host simplificada para leigos: só código + status; porta, backend,
  impressão digital e Registro (incl. Mbps) ficam em "Detalhes técnicos"
  (recolhido por padrão).
- ✅ **Smoke loopback validado em 2026-07-09** (ver 2.5.1): visual, código,
  aprovação, vídeo e teardown OK na mesma máquina.
- **Pendente:** teste em 2 máquinas (fluidez do canvas a 1080p, teclado/atalhos
  pelo browser, troca de monitor) — é a Etapa 1 do plano (seção 5).

### 3.7 ✅ Código de acesso autocontido (conectar só com o código) — 2026-07-08
- O host agora exibe UM código de 16 caracteres (`XXXX-XXXX-XXXX-XXXX`, base32
  Crockford) que embute IP + porta + segredo de 30 bits, com 2 bits de versão
  (preparando a Fase 8: ID de rendezvous no mesmo campo). O viewer digita só o
  código; campo `IP:porta` virou "Avançado" (sobrepõe o endereço embutido).
- IP detectado via crate `local-ip-address` (tabela de rotas do SO); override
  `advertised_ip` no config.toml para máquinas com várias interfaces/VPN. Sem
  detecção, o host avisa no Registro.
- Sem mudança de protocolo (`AuthRequest.session_code` = código completo;
  comparação constant-time como antes). Formato em `docs/protocol.md`;
  implementação em `crates/security/src/connect_code.rs`.
- **Pendente:** validar com duas máquinas (conectar só com o código; testar
  modo avançado; conferir IP detectado na máquina com VPN, se houver).

## 4. Como retomar o ambiente de teste (resumo)

Pré-requisitos (uma vez por máquina): Rust 1.91+, `rustup target add
wasm32-unknown-unknown` e `cargo install trunk tauri-cli --locked`; no Linux,
`libwebkit2gtk-4.1-dev`.

- **Linux:** binário release JÁ COMPILADO com o logging novo em
  `apps/controlis-app/src-tauri/target/release/controlis`
  (build: `cd apps/controlis-app/src-tauri && cargo tauri build --features
  wayland,real-capture --no-bundle`). Rodar com
  `RUST_LOG=info ./target/release/controlis 2>&1 | tee /tmp/controlis.log`.
- **Windows:** `git clone`/`git pull` da branch `development` (NÃO zipar —
  o repo está em github.com/DiogoBrazil/controlis), depois
  `cd apps\controlis-app\src-tauri && cargo tauri build --features
  real-capture --no-bundle`. Sem console no release: diagnóstico pela UI
  (Registro em "Detalhes técnicos").
- Smoke local com 2 instâncias na MESMA máquina: exportar
  `XDG_CONFIG_HOME`/`XDG_DATA_HOME` distintos por instância (ver 2.5.1).
- **VPS (Fase 8):** passo a passo completo em `deploy/README.md`
  (iroh-relay com ACME em 80/443 + QUIC 7824/udp; rendezvous em 8080/tcp).
  Config das duas máquinas (`~/.config/controlis/config.toml` no Linux,
  `%APPDATA%\controlis\controlis\config\config.toml` no Windows):

```toml
rendezvous_url = "http://rdv.SEUDOMINIO:8080"
relay_url = "https://relay.SEUDOMINIO"
```

## 5. Onde paramos exatamente (2026-07-09) e o que falta

**Funcionando e validado até aqui:**
- MVP LAN ponta a ponta nas duas direções (Linux Wayland ↔ Windows), teclado
  do host Windows e código autocontido validados (2026-07-08, na UI egui).
- UI nova Tauri v2 + Leptos: smoke loopback validado (2.5.1), com teardown
  limpo (sem freeze do gnome-shell) e H.264 fluindo.
- Logging do viewer completo (2.5.2) — pronto para diagnosticar teste remoto.
- Fase 8 implementada com testes locais verdes; deploy do VPS documentado e
  templetizado em `deploy/` (2.5.3) — **nunca testada fora da LAN**.

**Plano de testes aprovado** (detalhe em `~/.claude/plans/dazzling-tumbling-hickey.md`,
resumo abaixo). Sequência: cada etapa isola uma variável nova.

**➡️ PRÓXIMO PASSO — Etapa 1: teste LAN com 2 máquinas (UI nova).**
Preparo do Windows: git pull + `rustup target add wasm32-unknown-unknown` +
`cargo install trunk tauri-cli --locked` + build (seção 4). Roteiro nas DUAS
direções (checklist completo em `docs/test-matrix.md`):
1. Conectar só com o código; depois modo Avançado com IP manual.
2. Aprovação manual; fluidez do canvas a 1080p.
3. Teclado via browser (novo na UI Tauri): "ação já çê", símbolos com shift,
   Ctrl+C/V, Alt+Tab, Shift+setas.
4. Troca de monitor ao vivo.
5. **Medição da Fase 7:** movimento contínuo ~10 s → anotar `mídia (H264):
   X Mbps, Y fps` (aceite < 4 Mbps; anotar fps real — ver pendência 19 fps).
6. TOFU: reconexão passa; apagar `host_cert.der`/`host_key.der` no host →
   viewer deve BLOQUEAR por identidade alterada.
7. Robustez: derrubar Wi-Fi no meio → host volta a "aguardando", nenhuma
   tecla presa. Firewall UDP 21118 quando Windows for host (comando em 3.6).

**Etapa 2 — VPS (pode ser feita em paralelo, não depende das 2 máquinas):**
seguir `deploy/README.md`: DNS `relay.SEUDOMINIO` → IP do VPS; instalar
`iroh-relay` (`cargo install iroh-relay --features server --locked`) com
`deploy/iroh-relay.toml` (editar domínio) + unit systemd; build do
`controlis-server` no VPS + unit; liberar 80/tcp, 443/tcp, 7824/udp, 8080/tcp.
Smoke: `curl http://rdv.SEUDOMINIO:8080/healthz` → `ok`; TLS válido em
`https://relay.SEUDOMINIO`.

**Etapa 3 — teste internet (Fase 8), depois das etapas 1 e 2:**
`config.toml` das duas máquinas com `rendezvous_url`/`relay_url` (seção 4);
host mostra código v1 e loga "internet ativo"; primeiro teste ainda na LAN
(menos variáveis), depois viewer em rede diferente (ex.: hotspot 4G) conecta
só com o código; TOFU internet (reconexão passa, identidade divergente
bloqueia); `relay_url` inválido → host loga fallback e volta ao código LAN;
código v1 + endereço manual avançado ainda conecta por LAN.

**Para concluir o ciclo atual (depois das etapas):** registrar resultados e
Mbps/fps aqui no STATUS, marcar a `test-matrix.md`, decidir sobre o fps ~19
(3.6) e o TLS do rendezvous (3.6), e fechar o aceite da Fase 7.
