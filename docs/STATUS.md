# STATUS do Controlis — diário de evolução e pendências

> Última atualização: **2026-07-08** (noite: Fase 8 pronta para teste com Iroh + relay self-host; antes: UI migrada de egui para Tauri v2 + Leptos, teclado validado, código de acesso autocontido).
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
- **Pendente:** validar que o freeze do mutter (2.2) não reaparece.

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
- ⬜ Documentar/automatizar regra de firewall no Windows host (UDP).

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
- **Pendente:** validação visual/funcional pelo usuário + teste em 2 máquinas
  (fluidez do canvas a 1080p, teclado/atalhos pelo browser, troca de monitor).

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

Pré-requisitos (uma vez por máquina): Rust 1.91+, `rustup target add wasm32-unknown-unknown`
e `cargo install trunk tauri-cli`; no Linux, `libwebkit2gtk-4.1-dev`.

Para teste internet, configure nas duas máquinas:

```toml
rendezvous_url = "https://SEU-VPS:8080"
relay_url = "https://SEU-RELAY"
```

No VPS, rode o rendezvous com:

```bash
CONTROLIS_SERVER_BIND=0.0.0.0:8080 cargo run -p controlis-server
```

Rode o `iroh-relay` como processo separado apontando para o mesmo host/URL
publicado em `relay_url`.

- **Linux (host):** `cd apps/controlis-app/src-tauri &&
  cargo tauri build --features wayland,real-capture`
  depois `RUST_LOG=info ./target/release/controlis 2>&1 | tee /tmp/controlis-host.log`
- **Windows (na pasta do projeto):** `cd apps/controlis-app/src-tauri &&
  cargo tauri build --features real-capture`, depois o exe em
  `apps/controlis-app/src-tauri/target/release/`.
- Dev rápido na máquina local: `cargo tauri dev` (dispara o trunk sozinho).
- Fonte para levar a outra máquina: zipar SEM os `target/` (raiz,
  `apps/controlis-app/target` e `apps/controlis-app/src-tauri/target`) e sem
  `apps/controlis-app/dist`.

## 5. Onde paramos exatamente

O MVP LAN está **funcional de ponta a ponta nas duas direções** entre Linux
(Wayland/GNOME 46) e Windows, com o teclado do host Windows e a conexão só
com código **validados pelo usuário**. Em 2026-07-08 (noite) a **UI foi
migrada de egui para Tauri v2 + Leptos** (item 3.8): visual moderno/responsivo,
mesma lógica de sessão LAN. A Fase 8 agora adiciona o caminho internet por
Iroh/rendezvous sem remover o caminho LAN.

**Próxima sessão:** validar a UI nova localmente (visual + smoke loopback com
duas instâncias) e depois o teste com duas máquinas: fluidez do canvas a
1080p, teclado/atalhos vindos do browser, troca de monitor, aprovação,
Mbps no Registro, TOFU persistente (reconexão passa; certificado trocado
bloqueia) e teardown no Linux (fechar janela/Ctrl+C sem congelar o
gnome-shell). No Windows, instalar as ferramentas (`trunk`, `tauri-cli`,
target wasm) antes do build. Para a **Fase 8**, o próximo passo é o teste real:
VPS com `controlis-server` + `iroh-relay`, `config.toml` preenchido nas duas
máquinas e conexão fora da LAN usando só o código.
