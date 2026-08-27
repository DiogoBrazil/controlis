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

### Código de acesso autocontido

`AuthRequest.session_code` carrega o **código de acesso completo** exibido no
host. O código não é só o segredo: ele embute o alvo, para que o viewer digite
uma única string (sem precisar do IP separado).

Formato v0 (LAN):

- Payload de 80 bits: versão (2 bits, `0`) + segredo CSPRNG (30 bits) +
  IPv4 (32 bits) + porta (16 bits).
- Codificação base32 Crockford (`0-9` + letras sem I/L/O/U) → 16 caracteres,
  exibidos `XXXX-XXXX-XXXX-XXXX`. Leitura tolera minúsculas, separadores e as
  confusões `O→0`, `I/L→1`.
- O endereço embutido não é secreto; o segredo de 30 bits, combinado com uso
  único + backoff + aprovação manual, é a credencial. IPv6 fora de escopo.

Formato v1 (Fase 8 / internet):

- Payload de 80 bits: versão (2 bits, `1`) + segredo CSPRNG (30 bits) +
  ID de rendezvous (48 bits).
- O viewer usa o ID para consultar o servidor rendezvous e obter os metadados
  de alcance do host. O segredo continua viajando em `AuthRequest.session_code`
  e é verificado pelo host depois que o transporte E2E abre.
- O app conecta v1 via Iroh/rendezvous quando `rendezvous_url` e `relay_url`
  estão configurados. Se o viewer preencher endereço manual avançado, v1 usa
  esse `IP:porta` como fallback LAN e mantém o código completo como segredo.

A decodificação acontece toda no viewer (`crates/security/connect_code.rs`);
nada muda no fio além do tamanho da string.

### Rendezvous HTTP (Fase 8)

Contrato compartilhado em `crates/rendezvous`; servidor inicial em
`servers/controlis-server`.

| Rota | Corpo | Resposta | Observação |
|---|---|---|---|
| `GET /healthz` | — | `ok` | health check |
| `POST /v1/register` | `RegisterRequest` | `RegisterResponse` | registra `session_id`, endpoint e TTL; retorna token de posse |
| `POST /v1/refresh` | `RefreshRequest` | `RegisterResponse` | exige token e renova endpoint/TTL |
| `GET /v1/sessions/{session_id}` | — | `LookupResponse` | retorna endpoint se o registro ainda não expirou |
| `POST /v1/unregister` | `UnregisterRequest` | `204 No Content` | exige token e remove o registro |

O servidor não vê o segredo da sessão e não termina mídia/input. A implementação
guarda registros em memória, limita TTL a 120 s, aplica rate limit simples por
IP e o host renova o registro enquanto o código está válido.

### Transporte internet (Iroh)

- O caminho LAN continua usando Quinn + TLS self-signed com TOFU.
- O caminho internet usa Iroh 1.0 com ALPN `dev.controlis/session/1`.
- Host e viewer criam endpoints Iroh com `RelayMode::Custom`, sempre a partir de
  `relay_url`; relay público não é fallback padrão.
- O endpoint publicado no rendezvous contém `endpoint_id`, `relay_url` e
  endereços diretos observados. O viewer monta um `EndpointAddr` com esses dados
  e abre a mesma sessão de controle/mídia já usada no LAN.

### Pinning de identidade do alvo

O viewer persiste a impressão digital TLS do host por uma chave de alvo
(`peer_key`). No formato LAN atual, a chave é `lan:<ip:porta>` após aplicar o
endereço manual avançado, se usado. Na primeira conexão o fingerprint é salvo;
nas próximas, o transporte recebe esse fingerprint como pin esperado e bloqueia
certificados divergentes. No formato internet, a chave é `rendezvous:<id>` e a
identidade persistida é `iroh:<endpoint_id>`; mudança de endpoint conhecido é
bloqueada antes de abrir a sessão.

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
