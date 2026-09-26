# copilot-tui · `cockpit`

Cockpit de terminal para quem passa o dia no shell: relógio grande com a cidade atual, agenda do Google Calendar com alerta vermelho antes da reunião, um htop enxuto que pisca quando a máquina ameaça travar, worktrees do Claude com seletor que dá `cd` e as conversas do Claude em aberto.

A arquitetura e o roadmap estão em [`PLAN.md`](PLAN.md).

## Estado

| Fase | Entrega | Status |
|---|---|---|
| F0 | Loop, config, temas, layout responsivo, relógio grande, cidade/fuso por IP, relógios extras | ✅ |
| F1 | Sistema (CPU, memória, swap, disco, rede, ping, PSI) + detector de travamento | — |
| F2 | Worktrees + sessões do Claude + seletor com `cd` | — |
| F3 | Google Calendar (OAuth2) + alerta vermelho | — |
| F4 | Wi-Fi do host (WSL), speedtest, clima, mapa | — |
| F5 | `theme install`, hot-reload, `doctor` | — |

## Instalar e rodar

```sh
cargo install --path .     # instala o binário `cockpit` em ~/.cargo/bin
cockpit config init        # cria ~/.config/cockpit/config.toml comentado
cockpit
```

| Tecla | Ação |
|---|---|
| `Tab` / `Shift+Tab` | alterna o painel em foco |
| `t` / `T` | próximo / anterior tema (o último fica lembrado) |
| `x` | dispensa o aviso da barra de status |
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
