# Deploy da Fase 8 — VPS (rendezvous + iroh-relay)

O VPS é infraestrutura headless: o **rendezvous** (`controlis-server`) traduz o
código digitado em "onde está o host" e o **iroh-relay** repassa o tráfego
criptografado quando o NAT impede conexão direta. Nenhum tráfego é legível pelo
VPS (criptografia de ponta a ponta do Iroh).

## Pré-requisitos

- VPS Linux com IP público e Rust instalado (`rustup`).
- DNS: um subdomínio `relay.SEUDOMINIO` apontando para o IP do VPS
  (registro A). Para o rendezvous pode-se usar o mesmo host ou um
  `rdv.SEUDOMINIO` — ele roda em `http://...:8080`.
- Portas liberadas no firewall do VPS:

| Porta | Protocolo | Uso |
|-------|-----------|-----|
| 80    | tcp | iroh-relay: upgrade dos clientes + desafio ACME |
| 443   | tcp | iroh-relay: HTTPS/WebSocket (caminho principal) |
| 7824  | udp | iroh-relay: descoberta de endereço via QUIC |
| 8080  | tcp | controlis-server (rendezvous, http) |

> O relay é dono das portas 80/443 (TLS automático via Let's Encrypt embutido).
> Por isso o rendezvous fica em 8080 sem TLS neste primeiro deploy; colocar
> TLS nele (outro IP ou DNS-01) é hardening futuro.

## 1. iroh-relay

Instalar a versão 1.x (mesma família do cliente `iroh 1.0.2` do app):

```bash
cargo install iroh-relay --features server --locked
sudo cp ~/.cargo/bin/iroh-relay /usr/local/bin/
sudo mkdir -p /etc/iroh-relay
sudo cp deploy/iroh-relay.toml /etc/iroh-relay/config.toml
# Editar: trocar relay.SEUDOMINIO pelo subdomínio real.
sudo cp deploy/iroh-relay.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now iroh-relay
journalctl -u iroh-relay -f   # aguardar emissão do certificado ACME
```

Smoke: `curl -I https://relay.SEUDOMINIO` deve responder (200/404, com TLS
válido).

## 2. controlis-server (rendezvous)

Build no próprio VPS a partir do repositório:

```bash
git clone https://github.com/DiogoBrazil/controlis.git && cd controlis
cargo build --release -p controlis-server
sudo cp target/release/controlis-server /usr/local/bin/
sudo cp deploy/controlis-server.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now controlis-server
```

Smoke (de qualquer máquina):

```bash
curl http://rdv.SEUDOMINIO:8080/healthz          # → ok
```

## 3. config.toml das duas máquinas (host e viewer)

Linux: `~/.config/controlis/config.toml` — Windows:
`%APPDATA%\controlis\controlis\config\config.toml`

```toml
rendezvous_url = "http://rdv.SEUDOMINIO:8080"
relay_url = "https://relay.SEUDOMINIO"
```

Sem esses campos (ou com o relay fora do ar), o host cai para o código LAN —
comportamento esperado, registrado no log.
