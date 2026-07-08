# STATUS do Controlis — diário de evolução e pendências

> Última atualização: **2026-07-08** (correções pós-teste implementadas; aguardando validação com duas máquinas).
> Este arquivo é o ponto de retomada: o que está pronto, o que foi observado
> nos testes e o que falta revisar. Complementa o `PLANO-TECNICO.md` (plano) e
> o `TESTE-DUAS-MAQUINAS.md` (roteiro de teste).

## 1. Estado geral das fases

| Fase | Descrição | Estado |
|------|-----------|--------|
| 0–5  | Protocolo, transporte QUIC, captura, input, codec tiles+JPEG, sessões host/viewer, UI egui, storage | ✅ Prontas, 39 testes verdes, clippy limpo |
| 6    | Multi-monitor (origin_x/origin_y, troca ao vivo, seletor no viewer) | ✅ Pronta (e2e cobre troca de monitor) |
| 7    | H.264 via openh264 (feature `h264`, default; negociação no handshake, fallback JpegTiles; PROTOCOL_VERSION=2) | ✅ Implementada. ⚠️ Falta medir banda real (aceite: 30fps@1080p < 4 Mbps) |
| 8    | Rendezvous / relay / NAT traversal (acesso fora da LAN) | ❌ NÃO iniciada — próxima grande fase |
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

### 3.1 ✅ Teclado no host Windows: só números e maiúsculas — CORRIGIDO (validar)
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
- **Pendente:** validar no teste com duas máquinas (Windows host + Linux
  viewer): minúsculas/MAIÚSCULAS, "ação já çê", símbolos !@#, Ctrl+C/V.

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

### 3.5 ❌ Fase 8 — Rendezvous / Relay / NAT
- Hoje só funciona na mesma LAN. Próxima grande entrega para uso real
  (conceito AnyDesk: conectar por ID através da internet).

### 3.6 Melhorias de UX/robustez
- ✅ A UI do host agora mostra o backend ativo ("Backend: Wayland portal
  (PipeWire + EIS)" / "xcap + enigo"); fallback do portal aparece no Registro.
- ⬜ Documentar/automatizar regra de firewall no Windows host (UDP).

## 4. Como retomar o ambiente de teste (resumo)

- **Linux (host):**
  `cargo build --release -p controlis --features wayland,real-capture`
  depois `RUST_LOG=info ./target/release/controlis 2>&1 | tee /tmp/controlis-host.log`
- **Windows (Git Bash, na pasta do projeto):**
  `cargo build --release -p controlis --features real-capture`
  depois `./target/release/controlis.exe`
- Roteiro completo, logs a observar e firewall: `TESTE-DUAS-MAQUINAS.md`.
- Fonte para levar a outra máquina: zipar SEM `target/` (14 GB de artefatos;
  o fonte tem <1 MB): `zip -rq controlis-src.zip controlis -x "controlis/target/*"`.

## 5. Onde paramos exatamente

O MVP LAN está **funcional de ponta a ponta nas duas direções** entre Linux
(Wayland/GNOME 46) e Windows. Em 2026-07-08 foram implementados: o fix do
teclado no host Windows via mensagem `Text` + `Enigo::text()` (3.1, protocolo
v3), o log de banda no host (3.2), o endurecimento do teardown portal/EIS +
Ctrl+C gracioso (3.3) e o indicador de backend na UI (3.6). Workspace com
testes verdes e clippy limpo.

**Próxima sessão:** repetir o teste com duas máquinas (rebuild NAS DUAS pontas
— o protocolo subiu para v3) seguindo o checklist novo do `test-matrix.md`:
teclado no Windows host (minúsculas/acentos/símbolos/atalhos), leitura do
Mbps no Registro, e teardown no Linux (fechar app/Ctrl+C sem congelar o
gnome-shell). Validado isso, começa a **Fase 8** (rendezvous/relay/NAT).
