# copilot-tui · `cockpit`

Cockpit de terminal para quem passa o dia no shell: relógio grande com a cidade atual, agenda do Google Calendar com alerta vermelho antes da reunião, um htop enxuto que pisca quando a máquina ameaça travar, worktrees do Claude com seletor que dá `cd` e as conversas do Claude em aberto.

A arquitetura e o roadmap estão em [`PLAN.md`](PLAN.md).

## Estado

| Fase | Entrega | Status |
|---|---|---|
| F0 | Loop, config, temas, layout responsivo, relógio grande, cidade/fuso por IP, relógios extras | ✅ |
| F1 | Sistema (CPU, memória, swap, disco, rede, ping, PSI) + detector de travamento | ✅ |
| F2 | Worktrees + sessões do Claude + seletor com `cd` | ✅ |
| F3 | Google Calendar (OAuth2) + alerta vermelho | — |
| F4 | Wi-Fi do host (WSL), speedtest, clima, mapa | — |
| F5 | `theme install`, hot-reload, `doctor` | — |

## Instalar e rodar

```sh
cargo install --path .     # instala o binário `cockpit` em ~/.cargo/bin
cockpit config init        # cria ~/.config/cockpit/config.toml comentado
echo 'eval "$(cockpit shell-init zsh)"' >> ~/.zshrc   # para o Enter trocar de pasta
exec zsh
cockpit
```

Para atualizar depois de um `git pull`: `cargo install --path . --force`.

| Tecla | Ação |
|---|---|
| `Tab` / `Shift+Tab` | alterna o painel em foco |
| `t` / `T` | próximo / anterior tema (o último fica lembrado) |
| `x` | dispensa o aviso da barra de status |
| `↑↓` `j k` `g G` (Worktrees) | escolhe a worktree |
| `Enter` (Worktrees) | sai do cockpit e vai para a pasta da worktree |
| `C` (Worktrees) | abre `claude --continue` numa aba nova, já na worktree; o cockpit continua aberto |
| `/` (Worktrees) | filtra por nome ou branch (`Enter` aplica, `Esc` limpa) |
| `Enter` (Claude) | sai e vai para a pasta da conversa |
| `r` (Claude) | retoma uma conversa encerrada (`claude --resume`) numa aba nova |
| `i` | legenda dos símbolos do painel em foco |
| `←→` `↑↓` (painel Sistema) | escolhe a coluna (TOP CPU / TOP MEM) e o processo |
| `K` (painel Sistema) | encerra o processo com SIGTERM, depois de pedir confirmação |
| `?` | ajuda |
| `q` / `Ctrl+C` | sai |

Outros comandos: `cockpit theme list`, `cockpit config path`, `cockpit --theme nord`.

## Temas

Embutidos: `dracula`, `catppuccin-mocha`, `tokyo-night`, `nord`, `gruvbox-dark`, `one-dark`, `solarized-dark` e `terminal` (só cores ANSI, segue o tema do seu terminal).

Para criar ou instalar um tema, coloque um `.toml` em `~/.config/cockpit/themes/`. Tokens ausentes herdam de outros, e um arquivo com o mesmo `name` de um embutido o substitui:

```toml
name = "meu-tema"

[colors]
bg = "#1e1e2e"        # "#rrggbb", nome ANSI (red, lightblue…), índice 0-255 ou "reset"
fg = "#cdd6f4"
muted = "#6c7086"
border = "#45475a"
accent = "#cba6f7"
alert_bg = "#f38ba8"  # fundo do hero no alerta de reunião
```

Tokens: `bg fg muted border accent highlight ok warn alert alert_bg alert_fg spark gauge_low gauge_mid gauge_high clock`.

Em terminal sem truecolor, as cores são convertidas para a paleta de 256. O Windows Terminal é detectado por `WT_SESSION`.

## Sistema e detector de travamento

O painel Sistema mostra CPU e load, memória, swap, pressão (PSI), discos, rede da interface da rota padrão, ping e os maiores ofensores de CPU e memória, agrupados por nome.

| Métrica | Amarelo | Vermelho piscando |
|---|---|---|
| Memória | > 85 % | > 93 % |
| Swap (o amarelo só entra se ela estiver subindo) | > 50 % | > 80 % |
| PSI memória (% do tempo com tarefas travadas esperando RAM) | > 5 | > 20 |
| CPU média de 10 s | > 90 % | > 98 % |
| Disco | > 90 % | > 97 % |

No vermelho, a métrica e os maiores ofensores piscam, a borda do painel fica vermelha e o motivo aparece também na barra de status, que continua visível no modo compacto. Para baixar, o valor precisa cair 3 pontos abaixo do limiar, o que evita pisca-pisca. Tudo isso é configurável em `[system.danger]`.

O ping mede o tempo de conexão TCP a `1.1.1.1:443`, porque o WSL vem com ICMP sem root desligado (`net.ipv4.ping_group_range = 1 0`).

## Worktrees e conversas do Claude

O painel Worktrees varre as pastas de `[worktrees].workspaces` (padrão: `~`, dois níveis) atrás de repositórios git e lista as worktrees de cada um: branch, arquivos alterados (`✎`), commits à frente e atrás do upstream (`↑↓`), idade do último commit, tamanho em disco e as conversas do Claude abertas nela (`◆` esperando você, `⠿` trabalhando, `$` rodando shell, `●` parada).

O painel Claude lista as conversas abertas (lidas de `~/.claude/sessions`), com as que esperam você primeiro, e as encerradas nas últimas 24 h, que o `r` retoma. Cada conversa entra na worktree em que o Claude está de fato trabalhando, lida da transcrição, mesmo que ele tenha sido aberto na raiz do repositório.

Um programa não consegue mudar a pasta do shell que o abriu. Por isso o `Enter` depende da função instalada pelo `shell-init` (zsh, bash ou fish): o cockpit grava a pasta escolhida num arquivo temporário e a função faz o `cd` quando ele sai. Sem a função, o `Enter` copia o comando `cd …` para a área de transferência e mostra como ligá-la.

### Tokens e limites do plano

No topo do painel Claude ficam os limites do plano, os mesmos do `/usage`: a sessão de 5 h e a semana, com o percentual usado e quando cada um reinicia. Para isso o cockpit lê o login OAuth que o Claude Code guarda em `~/.claude/.credentials.json` e consulta o mesmo endpoint que ele, a cada 3 min. O token só é lido e enviado para `api.anthropic.com`: o cockpit nunca o renova nem grava. Com o login vencido, o painel pede para abrir o `claude`, que renova sozinho. Com API key não existe limite de plano. Desligue com `[claude].usage_limits = false`.

Cada conversa mostra o gasto da rodada, desde o seu último pedido: `⟳ rodando há 7m` enquanto trabalha, `última rodada` quando para. São três números:
- **saída:** tokens gerados, incluindo raciocínio.
- **processados:** tudo que passou pelas chamadas da rodada, com o cache lido.
- **contexto:** o tamanho da conversa na última chamada.

O gasto dos subagentes disparados na rodada entra na soma. A transcrição é lida de forma incremental: o arquivo inteiro na primeira vez e, depois, só o que foi acrescentado.

O `C` e o `r` abrem uma aba nova em vez de fechar o cockpit. O terminal é detectado: tmux (janela nova), Windows Terminal pelo WSL (aba nova na mesma janela, via `wt.exe` e `wsl.exe --cd`), kitty, WezTerm ou gnome-terminal. O comando roda no seu shell de login interativo, para ter o mesmo PATH de sempre, e a aba continua aberta quando o Claude termina. Para outro terminal, defina `[terminal].new_tab` no config. Se nenhum for reconhecido, os dois voltam ao comportamento antigo: saem e rodam o Claude no shell atual pelo wrapper.

Custo: o `git status` roda a cada 30 s (`refresh_secs`) no modo rápido, sem contar arquivos não rastreados; a contagem completa vai a cada 5 min ou quando algo muda no `.git`. O tamanho em disco é recalculado a cada 10 min em segundo plano.

## Cidade e fuso

Sem `location.city` no config, a cidade vem do IP (ipapi.co, com ipinfo.io de reserva), refeita a cada hora e guardada em cache para a abertura ser instantânea. Geolocalização por IP costuma errar a cidade (o provedor pode sair em outra capital), então fixe a sua no config:

```toml
[location]
city = "Rio de Janeiro"
country = "Brazil"
timezone = "America/Sao_Paulo"
```

## Desenvolvimento

```sh
cargo test
cargo clippy --all-targets
COCKPIT_LOG=debug cargo run      # log em ~/.local/state/cockpit/cockpit.log
```
