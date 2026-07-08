# STATUS do Controlis — diário de evolução e pendências

> Última atualização: **2026-07-07** (noite, após teste com duas máquinas).
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

## 3. Bugs e pendências abertas (ordem sugerida de ataque)

### 3.1 🐛 Teclado no host Windows: só números e maiúsculas
- Sintoma: com Windows como host, digitação vinda do viewer só produz números
  e letras maiúsculas (minúsculas não saem corretamente).
- Caminho do código: viewer envia `KeyCode::Unicode(ch)` press/release
  (`apps/controlis/src/input_map.rs`, eventos `Text`); host Windows injeta via
  enigo em `crates/input/src/enigo_backend.rs:128`
  (`KeyCode::Unicode(c) => Key::Unicode(c)`).
- Hipóteses a investigar:
  1. No Windows, `enigo::Key::Unicode` faz press/release via `VkKeyScanW`, cujo
     retorno inclui um flag de shift no byte alto — se o estado de shift for
     aplicado/ignorado errado, o case sai errado.
  2. Interação com o diff de modificadores do viewer (`diff_modifiers` envia
     Shift press/release separadamente) — pode estar "grudando" Shift no host.
  3. Alternativa de correção: para texto, usar `Enigo::text()` (SendInput com
     KEYEVENTF_UNICODE) em vez de press/release de `Key::Unicode` — injeta o
     caractere exato, independente de layout/shift.
- Testar também: acentos/ç no Windows host, e símbolos (!@#...).

### 3.2 ⚠️ Medição de banda do H.264 (aceite da Fase 7)
- Nunca medido em captura real: alvo 30fps @ 1080p < 4 Mbps.
- Sugestão: logar bytes/s enviados no host durante sessão real entre as duas
  máquinas (ou observar via `nload`/`iftop`).

### 3.3 ⚠️ Teardown da sessão portal/EIS no Linux (relacionado ao freeze 2.2)
- Garantir Stop da sessão do portal e disconnect do cliente ei em TODOS os
  caminhos de saída (fim de sessão, erro, panic, Ctrl+C).
- Objetivo: nunca mais deixar o mutter pendurado.

### 3.4 Limitações conhecidas do teclado EIS (Linux host)
- AltGr, dead-keys e acentos compostos ainda não suportados
  (`eis_input.rs`, mapa keysym→keycode). Não bloqueou o teste, mas revisar.

### 3.5 ❌ Fase 8 — Rendezvous / Relay / NAT
- Hoje só funciona na mesma LAN. Próxima grande entrega para uso real
  (conceito AnyDesk: conectar por ID através da internet).

### 3.6 Melhorias de UX/robustez observadas nos testes
- Fallback do backend Wayland→enigo é silencioso (só `tracing::error`);
  considerar avisar na UI qual backend de input está ativo.
- Documentar/automatizar regra de firewall no Windows host (UDP).

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
(Wayland/GNOME 46) e Windows, com vídeo e mouse perfeitos. O único defeito
funcional aberto é o teclado no host Windows (item 3.1) — é o primeiro item da
próxima sessão de trabalho, seguido da medição de banda H.264 (3.2) e do
endurecimento do teardown Wayland (3.3). Depois disso, Fase 8.
