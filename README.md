# Controlis

Aplicativo desktop de acesso remoto em Rust. Um único binário com dois modos:

- **Host** — este computador é controlado (mostra um código de sessão).
- **Viewer** — este computador controla outro (exibe a tela e envia mouse/teclado).

Conexão direta na LAN, cifrada fim a fim com QUIC/TLS 1.3, autenticação por código
de sessão de uso único e aprovação manual no host. Sem servidor externo no MVP.

Veja o planejamento completo em [`PLANO-TECNICO.md`](PLANO-TECNICO.md).

## Estado atual (MVP)

Ciclo completo funcionando: captura → codec → transporte (QUIC) → decodificação
→ exibição, e input (mouse/teclado) do viewer aplicado no host. Interface em
egui com indicador de sessão ativa e botão de encerrar.

Vídeo: **H.264** (OpenH264, compilado do fonte — feature `h264`, ligada por
padrão) negociado no handshake, com fallback automático para **dirty tiles +
JPEG** quando um dos lados não o suporta. Para compilar sem o codec (dispensa
compilador C++): `cargo run -p controlis --no-default-features`.

Por padrão o host transmite um **padrão sintético** (um retângulo em movimento), o
que permite rodar e testar todo o pipeline em qualquer máquina, sem display nem
bibliotecas de captura do sistema.

## Compilar e rodar

```sh
cargo run -p controlis
```

Para capturar a **tela real** (backend `xcap`):

```sh
cargo run -p controlis --features real-capture
```

No Linux o backend `xcap` exige dependências de build do sistema:

- `libpipewire-0.3-dev` (e afins) — a captura via Wayland usa o portal ScreenCast
  + PipeWire. Em X11 o backend usa XCB/SHM.
- Headers builtin do Clang para o `bindgen` (que gera os bindings do PipeWire).
  Se aparecer `fatal error: 'stdbool.h' file not found`, instale:

  ```sh
  sudo apt install libclang-common-18-dev   # combine a versão com a sua libclang
  ```

  Alternativa sem sudo (aponta o bindgen para os headers do gcc):

  ```sh
  export BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/gcc/x86_64-linux-gnu/13/include"
  ```

**Nota sobre Wayland nativo:** o backend `real-capture` (xcap + enigo) usa
X11/XTest para input e só alcança janelas XWayland. Para captura **e** input
nativos no Wayland, use o backend de portal:

```sh
cargo run -p controlis --features wayland
```

Ele cria uma sessão de portal combinada (ScreenCast + RemoteDesktop) via
`xdg-desktop-portal` — você verá o diálogo de consentimento do sistema — faz
captura por PipeWire e injeção de mouse/teclado por **libei/EIS**, na mesma sessão
(necessário para posicionamento absoluto do ponteiro). No GNOME 45+ os métodos
`NotifyPointer*`/`NotifyKeyboard*` do portal não funcionam mais; o input vai por
`ConnectToEIS` + libei (crates `reis`/`xkbcommon`). Testado em GNOME 46/Wayland:
captura, mouse e teclado funcionam. Exige `libpipewire-0.3-dev` e `libgbm-dev`.

Para validar o fluxo de portal isoladamente (consentimento, captura de 1 quadro e
um clique de teste), rode o probe:

```sh
cargo run -p wayland-portal --features enabled --example wayland_probe
```

Limitações conhecidas: caracteres via AltGr / dead-keys / acentos compostos podem
não sair corretos (dependem de níveis de modificador que o viewer não envia); o
suporte a EIS varia entre compositores (validado no GNOME 46). O modo sintético
padrão continua sendo o caminho para exercitar o ciclo completo sem hardware.

A injeção de mouse/teclado usa `enigo` (SendInput no Windows, XTest no X11). No
Wayland nativo a injeção depende do portal RemoteDesktop e ainda não faz parte
deste MVP (ver Fase 9 do plano).

## Testes

```sh
cargo test --workspace      # unitários + integração (inclui o ciclo completo headless)
cargo clippy --workspace
```

O teste `session-host/tests/end_to_end.rs` exercita autenticação, streaming de um
quadro, envio de input e liberação de teclas na desconexão — tudo sem display.

## Estrutura

Monorepo Cargo. Cada crate tem uma responsabilidade única e os backends de SO
ficam atrás de traits (`ScreenCapturer`, `InputInjector`) para permitir troca e
teste. Detalhes em [`docs/architecture.md`](docs/architecture.md).

```
crates/
  protocol/        mensagens, serialização (serde+postcard), framing, versão
  transport/       QUIC (quinn) + TLS 1.3 + identidade e TOFU de fingerprint
  capture/         trait ScreenCapturer (backends sintético e xcap)
  input/           trait InputInjector (backend enigo) + coordenadas normalizadas
  codec/           encode/decode: H.264 (OpenH264) e dirty tiles + JPEG
  security/        código de sessão, rate limiting/backoff, comparação constant-time
  storage/         config TOML + SQLite (logs de conexão, peers conhecidos)
  session-host/    orquestração do host (auth, captura→envio, aplicar input)
  session-viewer/  orquestração do viewer (auth, receber/decodificar, enviar input)
  wayland-portal/  backend Wayland: portal (ashpd) + PipeWire (feature `enabled`)
apps/
  controlis/       binário único com UI egui
docs/              protocolo, arquitetura, matriz de testes
```

## Segurança

- Canal sempre cifrado (QUIC/TLS 1.3); não existe modo em claro.
- Código de sessão aleatório (CSPRNG), uso único, comparação em tempo constante.
- Aprovação manual da conexão no host (configurável).
- Backoff exponencial por IP contra força bruta.
- Trust-on-first-use: o viewer fixa a impressão digital do certificado do host.
- Indicador visível de sessão ativa e botão de encerrar sempre acessível.
- Liberação de todas as teclas/botões ao desconectar (proteção contra tecla presa).
- Log local de conexões em SQLite.

Nenhuma técnica de stealth, persistência oculta ou bypass de permissões do SO.

## Licença

MIT OR Apache-2.0.
