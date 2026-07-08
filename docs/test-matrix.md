# Matriz de testes manuais

Testes automatizados cobrem protocolo, serialização, segurança, codec (dirty
tiles), conversão de coordenadas, handshake/TOFU e o ciclo completo headless
(`cargo test --workspace`). Esta matriz cobre o que exige display e SO reais.

## Combinações de plataforma (MVP)

Executar com `--features real-capture` em ambas as pontas.

| Host | Viewer | Status |
|---|---|---|
| Linux X11 | Linux X11 | pendente |
| Linux X11 | Windows | pendente |
| Windows | Linux X11 | pendente |
| Windows | Windows | pendente |

Wayland (host) entra na Fase 9.

## Checklist por combinação

- [ ] Host exibe código e impressão digital; viewer conecta com código correto.
- [ ] Código errado é rejeitado; 5 erros disparam backoff (mensagem de espera).
- [ ] Aprovação manual: host mostra o pedido; Aceitar/Recusar funcionam.
- [ ] Vídeo aparece no viewer; tela parada consome banda próxima de zero.
- [ ] Ponteiro acerta o alvo com host e viewer em resoluções/DPI diferentes.
- [ ] Clique esquerdo/direito/meio; arrastar; scroll.
- [ ] Digitação pt-BR/ABNT2 (ex.: "ação já çê") chega correta.
- [ ] Atalhos com modificador (Ctrl+C, Alt+Tab) funcionam.
- [ ] Encerrar pela host libera o input e gera novo código.
- [ ] Queda de rede: host volta ao estado "aguardando"; nenhuma tecla fica presa.
- [ ] TOFU: reconexão com o mesmo host passa; certificado trocado gera alerta.

## Ambiente de captura/input em CI

- Linux: `Xvfb` permite testar captura X11 e injeção XTest de ponta a ponta
  (injetar tecla → capturar efeito). Marcar esses testes como `#[ignore]` no CI
  headless padrão e rodá-los no job dedicado com display virtual.
