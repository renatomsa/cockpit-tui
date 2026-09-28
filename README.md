# cockpit-tui

Cockpit de terminal para quem passa o dia no shell. Numa tela só:
- relógio grande com cidade, clima e mapa;
- próxima reunião do Google Calendar, com alerta vermelho antes de começar;
- saúde da máquina: CPU, memória, disco e rede, com aviso quando ameaça travar;
- containers do Docker;
- worktrees com seletor que dá `cd`;
- conversas do Claude Code, com tokens por rodada e limites do plano.

Escrito em Rust. Parado, usa menos de 1 % de CPU e cerca de 30 MB de memória.

## Instalar

```sh
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --git ssh://git@github.com/renatomsa/cockpit-tui.git
cockpit config init                                    # ~/.config/cockpit/config.toml comentado
echo 'eval "$(cockpit shell-init zsh)"' >> ~/.zshrc    # para o Enter trocar de pasta (bash e fish também)
exec zsh && cockpit
```

O repositório é privado. A variável `CARGO_NET_GIT_FETCH_WITH_CLI` faz o cargo usar o `git` do sistema, com a sua chave SSH do GitHub. Para não repeti-la, ponha `[net]` com `git-fetch-with-cli = true` em `~/.cargo/config.toml`. Para usar HTTPS em vez de SSH, rode `gh auth setup-git` uma vez e use `--git https://github.com/renatomsa/cockpit-tui`.

- **Atualizar:** o mesmo comando com `--force`.
- **A partir de um clone:** `cargo install --path .`.
- **Ver o que falta configurar:** `cockpit doctor`.

## Painéis

| Painel | O que mostra |
|---|---|
| **Relógio** | cidade (pelo IP ou fixa no config) e relógios extras; próxima reunião; clima e próximas horas (Open-Meteo); mapa-múndi com a cidade |
| **Agenda** | reuniões das próximas 48 h, com link da chamada |
| **Docker** | containers rodando (o `docker ps`, sem `-a`): saúde, portas, projeto compose |
| **Worktrees · Claude** | duas abas: worktrees dos repositórios, com branch, alterações, ↑↓, idade e tamanho; conversas do Claude abertas e encerradas há pouco, com tokens da rodada e os limites de 5 h e da semana |
| **Sistema** | caixas de CPU, Memória, Disco e Rede com detalhe por tipo de uso; Wi-Fi e tráfego do Windows no WSL; maiores ofensores de CPU e memória; especificações da máquina |

Alertas:
- **Reunião:** a borda fica amarela a 15 min. De 5 min antes a 5 min depois do início, o relógio inteiro fica vermelho e piscando.
- **Sistema:** amarelo no aviso e vermelho em negrito no crítico, para memória, swap, pressão de memória (PSI), CPU sustentada e disco. Os limites ficam em `[system.danger]`.

A tecla `i` mostra a legenda dos símbolos do painel em foco.

## Teclas

| Onde | Tecla | Ação |
|---|---|---|
| geral | `Tab` / `Shift+Tab` | troca o painel em foco |
| | `t` / `T` | próximo / anterior tema |
| | `i` · `?` · `x` | legenda · ajuda · dispensa o aviso da barra |
| | `q` / `Ctrl+C` | sai |
| alerta de reunião | qualquer tecla · `z` · `o` | para o alerta · adia 5 min · abre a chamada |
| Agenda | `o` · `y` | abre a chamada · copia o link |
| Docker | `K` · `X` | para o container · derruba tudo (`compose down --remove-orphans` por projeto), com confirmação |
| Worktrees | `← →` | troca entre as abas Worktrees e Claude |
| | `Enter` · `C` | vai para a pasta · abre `claude --continue` numa aba nova |
| | `/` · `D` | filtra · exclui a worktree (dois avisos: `y` e depois Enter) |
| Claude | `Enter` · `r` | vai para a pasta · retoma uma conversa encerrada numa aba nova |
| Sistema | `s` | teste de velocidade (download e upload) |
| | `← →` `↑↓` · `K` | escolhe processo · encerra com SIGTERM, com confirmação |

## Comandos

| Comando | O que faz |
|---|---|
| `cockpit doctor` | confere cada peça e diz como resolver o que falta |
| `cockpit theme list` / `install <arquivo ou URL>` / `remove <nome>` | gerencia temas |
| `cockpit config init` / `config path` | cria o config / mostra onde ficam config, temas, estado e log |
| `cockpit calendar add <endereço iCal>` / `list` / `remove <n>` | conecta a agenda pelo endereço iCal |
| `cockpit auth google` | conecta o Google Calendar por OAuth (alternativa) |
| `cockpit shell-init zsh\|bash\|fish` | função que faz o Enter trocar de pasta |
| `cockpit --demo-agenda` | reuniões de exemplo, para ver o alerta sem o Google |

## Configuração

Tudo fica em `~/.config/cockpit/config.toml`, cujo modelo está em [`config.example.toml`](config.example.toml). O arquivo é **recarregado ao salvar**:
- **Na hora:** tema, relógio, layout, idioma e tempos do alerta.
- **Ao reabrir:** as seções lidas na abertura. Quando você muda uma delas, o cockpit avisa.
- **Com erro:** o config não é aplicado, e o cockpit mantém o anterior.

Ajustes mais comuns:

```toml
theme = "dracula"                 # dracula, catppuccin-mocha, tokyo-night, nord, gruvbox-dark, one-dark, solarized-dark, terminal

[location]                        # o IP costuma errar a cidade
city = "Rio de Janeiro"
timezone = "America/Sao_Paulo"

[[clock.extra]]
label = "Orlando"
tz = "America/New_York"

[worktrees]
workspaces = ["~"]                # onde procurar repositórios
```

**Temas** são arquivos `.toml` com tokens de cor (`bg fg muted border accent highlight ok warn alert alert_bg alert_fg spark gauge_low gauge_mid gauge_high clock`). O que faltar herda de outro token. `cockpit theme install` valida o tema antes de instalar, e um cockpit aberto já o mostra no ciclo do `t`.

## Google Calendar

O jeito mais simples é o **endereço iCal**. Não precisa de Google Cloud nem de OAuth:

1. No Google Calendar (web), abra **Configurações › sua agenda › Integrar agenda** e copie o **"Endereço secreto no formato iCal"**.
2. Rode `cockpit calendar add '<endereço>'`. Ele confere o endereço e guarda.

A agenda é relida a cada 5 min. Os eventos recorrentes são expandidos, e ficam de fora os cancelados e os que você recusou; os de dia inteiro aparecem, mas nunca disparam alerta. Vale qualquer feed ou arquivo `.ics`, e dá para cadastrar vários (`calendar list`, `calendar remove <n>`). O endereço fica em `~/.local/state/cockpit/calendars.json`, com permissão 0600, e não no config. Quem tiver o endereço lê a sua agenda: se ele vazar, gere outro no Google.

**Se a empresa esconder o endereço secreto** (o Workspace pode desligar essa opção), use o OAuth, que é somente leitura:

1. Em [console.cloud.google.com](https://console.cloud.google.com), crie um projeto e ative a **Google Calendar API**.
2. Em **Google Auth Platform**, configure o público-alvo como **Interno** e crie um cliente **App para computador**. Baixe o JSON para `~/.config/cockpit/google_client.json`.
3. Rode `cockpit auth google`.

A leitura do `.ics` segue a abordagem do [meridian](https://github.com/victorlcampos/meridian) (MIT).

## Privacidade e custo

- **Limites do plano:** o cockpit lê o login do Claude Code em `~/.claude/.credentials.json` e consulta `api.anthropic.com` a cada 3 min. O token nunca é renovado nem gravado. Para desligar: `[claude].usage_limits = false`.
- **Rede:** o teste de velocidade só roda quando você aperta `s` e baixa 25 MB e envia 10 MB para a Cloudflare. A cidade vem de ipapi.co ou ipinfo.io, e o clima, da Open-Meteo.
- **WSL:** um PowerShell fica aberto para ler o Wi-Fi do Windows (cerca de 0,5 % de CPU e 80 MB). Para desligar: `[network].wsl_host = false`.
- **Git:** o `git status` roda a cada 30 s com `--no-optional-locks`, então nunca disputa o índice com o seu git. O tamanho das worktrees é recalculado a cada 10 min.

## Desenvolvimento

```sh
cargo test
cargo clippy --all-targets
COCKPIT_LOG=debug cargo run      # log em ~/.local/state/cockpit/cockpit.log
```
