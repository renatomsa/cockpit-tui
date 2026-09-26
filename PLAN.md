# Plano — `cockpit`: TUI de cockpit do desenvolvedor (Rust)

## Contexto

Você quer ficar o dia inteiro no terminal, com um painel fixo que junte o que hoje fica espalhado: hora e cidade, próximas reuniões do Google Calendar (com **alerta vermelho** quando uma reunião se aproxima, como no print), a saúde da máquina (um htop enxuto que **pisca em vermelho** quando a máquina ameaça travar), as **worktrees do Claude** dos projetos, com seletor que dá `cd`, e as **conversas do Claude em aberto**. Tem que ser customizável (temas instaláveis: dracula etc.) e consumir quase nada de CPU.

Decisões já tomadas:
- Calendar via **OAuth2 próprio** (Desktop client no Google Cloud).
- Troca de worktree por **wrapper de shell + `cd`** (padrão yazi/broot).
- Cidade por **geolocalização por IP**, com override no config.
- Escopo desta etapa: **só o plano** (implementação por fases, seção 5).

O que o ambiente mostrou (usado no desenho):
- Máquina: **WSL2 / Ubuntu 24.04**, zsh, `cargo 1.88`, `powershell.exe` acessível.
- Worktrees do Claude ficam em `<repo>/.claude/worktrees/<nome>` (ex.: `~/v360/.claude/worktrees/processable-file-cfs`), visíveis em `git worktree list`.
- Sessões vivas do Claude: `~/.claude/sessions/<pid>.json` com `pid`, `procStart`, `sessionId`, `cwd`, `kind` (`interactive`|`bg`), `status` (`busy`|`idle`|`waiting`|`shell`), `name`, `updatedAt`. Há 67 arquivos e só ~5 processos vivos, então **arquivo parado precisa ser filtrado**.
- Transcrições: `~/.claude/projects/<cwd-codificado>/<sessionId>.jsonl` (cwd com `/` e `.` virando `-`). As linhas `ai-title`/`agent-name` dão o título, e `away_summary` dá o resumo de onde a conversa parou.

---

## 1. Arquitetura para "rodar liso"

**Princípio:** a thread de UI nunca faz I/O. Cada fonte de dados é um *provider* assíncrono que publica o **último snapshot** num `tokio::sync::watch`. Com semântica de último valor, a fila nunca cresce e um provider lento não trava ninguém. A UI desenha só quando algo mudou ou quando o relógio vira o segundo.

```
                ┌──────────── tokio runtime (2 workers) ────────────┐
 crossterm      │  providers (tasks independentes, com backoff)      │
 EventStream ─┐ │  system  net  ping  geo  weather  calendar         │
              │ │  git/worktrees  claude-sessions  wsl-host           │
              │ │        │ watch::Sender<Snapshot> (último valor)     │
              ▼ │        ▼                                            │
        ┌─────────────────────────┐   Action   ┌──────────────────┐  │
        │ loop principal select!  │──────────▶ │ AppState (Elm)   │  │
        │ key | tick 1s | changed │            │ update() puro    │  │
        └─────────────────────────┘            └────────┬─────────┘  │
                                         dirty? ─▶ ratatui draw (diff)│
                └─────────────────────────────────────────────────────┘
 trabalho pesado (git status, du) → spawn_blocking + Semaphore(2)
```

- **Padrão Elm:** `Event → Action → update(&mut AppState) → view(&AppState)`. O `update` é puro e testável. O relógio entra por uma trait `Clock` injetável, para testar o alerta com hora falsa.
- **Frame rate:** o tick de 1 s é alinhado à virada do segundo (`sleep_until(próximo_segundo)`). Durante o blink (alerta ou perigo) sobe para 2 Hz, e tecla ou snapshot novo forçam redraw. Como o ratatui só envia ao terminal as células que mudaram, cada frame custa microssegundos.
- **Isolamento de falhas:** provider que falha fica com o último dado bom, o painel mostra `⚠ offline há 2 min` e o provider tenta de novo com backoff exponencial (máximo de 5 min). Um panic hook restaura o terminal sempre.
- **Cadências:**

| Fonte | Frequência | Como |
|---|---|---|
| CPU / mem / swap | 1 s | `sysinfo`, refresh só de cpu+memory |
| Processos (top) | 2 s | `ProcessRefreshKind` mínimo (cpu, mem, nome) |
| PSI (pressão) | 1 s | `/proc/pressure/{cpu,memory,io}` |
| Rede ↓↑ | 1 s | delta de `/proc/net/dev` |
| Ping | 5 s | ICMP sem privilégio, ou TCP connect em 1.1.1.1:443 como fallback |
| Discos | 30 s | `sysinfo::Disks` (`/`, `/mnt/c`) |
| Calendar | 60 s | API v3 incremental. A contagem regressiva é recalculada localmente a cada segundo |
| Worktrees | inotify em `.git/worktrees` + 30 s | `git worktree list --porcelain`, `git status` |
| Tamanho das worktrees | 10 min, baixa prioridade | walkdir em `spawn_blocking` |
| Sessões Claude | inotify em `~/.claude/sessions` (debounce 250 ms) | leitura tolerante |
| Geo / fuso | início + 1 h | ip-api/ipinfo |
| Clima (opcional) | 15 min | Open-Meteo (sem chave) |

- **Metas:** menos de 1 % de CPU parado, RSS abaixo de 40 MB, início em menos de 150 ms, nenhum frame acima de 2 ms.
- **Build de release:** `lto = "thin"`, `codegen-units = 1`, `strip = true`, `opt-level = 3`.

## 2. Layout

Layout largo (a partir de 160 colunas):
```
┌ HERO ─────────────────────────────────────────┬ SISTEMA (htop-lite) ──────┐
│ Rio de Janeiro 15:55 │ Orlando 14:55           │ CPU ▕████▌   ▏ 34% ld 1.2 │
│        Rio de Janeiro · Brazil                 │ ▁▂▅▇▅▂ por núcleo          │
│      ██ ████ ██ ████ ██ ████    [mapa braille] │ MEM ▕██████▌ ▏ 71% 11/16G  │
│ ▶ Conhecimento fiscal… em 4 min  (Meet: o)     │ SWP ▕█▏       6%           │
│ sexta, 25 set 2026 · UTC-03:00 · ☀ 25°C 45%    │ DSK / 62%  /mnt/c 81%      │
├ AGENDA ───────────────────────────────────────┤ NET ↓12.3M ↑1.1M ping 18ms │
│ 16:00 Daily        · 17:30 Review PR  · …      │ WiFi "Casa" 87% (host)     │
├ WORKTREES ─────────────────────────────────────┤ TOP CPU      TOP MEM       │
│ grove 29 wt  ◆1 esperando  ⠿10 trabalhando    │ node 88%     code 2.1G     │
│ ▸ v360  ~/v360                  4 wt · 1.2 GB  │ cargo 45%    chrome 1.8G   │
│   ├ processable-file-cfs  feat/… ●2 ↑1  ◆      ├ CLAUDE ────────────────────┤
│   └ build-s4-service-…    wt-…   ✓     ⠿      │ ◆ processable-file-cfs 2m  │
│ ▸ agentictreelo ~/Workspace/…   2 wt · 349 MB  │   "rebase pronto, falta…"  │
└────────────────────────────────────────────────┴───────────────────────────┘
 Tab foco · ↑↓/jk · / filtro · Enter cd · r resume · t tema · z soneca · q sair
```
- **Responsivo:** abaixo de 160 colunas o mapa some. Abaixo de 110 os painéis empilham (hero compacto, depois agenda, sistema, worktrees e claude), e abaixo de 30 linhas fica só o hero com a barra de status.
- **Relógio grande:** fonte própria 5×7 de blocos (`ui/bigclock.rs`), para ter controle total de tamanho e cor. Escala 2× quando sobra espaço.
- **Mapa:** `ratatui::widgets::canvas::Map` (`MapResolution::High`) em braille, com a cidade marcada pela lat/lon da geolocalização.

## 3. Módulos (funcionalidade → implementação)

### 3.1 Hero + alerta de agenda
- **Máquina de estados** `AlertState`, testável:

| Estado | Quando entra | Visual |
|---|---|---|
| `Calm` | padrão | cores normais |
| `Soon` | faltam até 15 min | borda `warn` |
| `Imminent` | faltam até 5 min, ou a reunião começou há até 5 min | fundo do hero todo em `theme.alert` (vermelho) com o texto em `alert_fg`, como no print |
| `Snoozed(até)` | tecla `z` | alerta suspenso por 5 min |
| `Dismissed(evento_id)` | qualquer tecla | alerta desligado só para aquele evento |

- No `Imminent`, a linha do evento pisca em 2 Hz e o rodapé mostra `qualquer tecla parar · z soneca 5 min`.
- Todos os limites ficam no config. Eventos de dia inteiro e convites recusados nunca disparam alerta.
- **Relógios secundários** vêm do config (ex.: Orlando), e o fuso da cidade atual vem da geolocalização (`chrono-tz`).

### 3.2 Google Calendar (OAuth2)
- `cockpit auth google` faz o fluxo *installed app* com PKCE e redirect de loopback em `127.0.0.1:<porta>` (o `localhostForwarding` do WSL2 entrega o callback). O navegador abre via `wslview`, `cmd.exe /c start` ou `xdg-open`, e o fallback é imprimir a URL e colar o código.
- Escopo `calendar.readonly`. O `client_secret.json` vem do caminho no config, e o token vai para `~/.local/share/cockpit/google_token.json` com permissão `0600`, com refresh automático.
- As chamadas são REST direto via `reqwest`, sem a crate `google-calendar3`, que é pesada:
  1. `calendarList.list` para descobrir as agendas.
  2. `events.list` por agenda, com `singleEvents=true`, `orderBy=startTime`, `timeMin=agora` e `timeMax=+48h`.
  3. Os eventos são mesclados, e ficam de fora os que `attendees[self].responseStatus == declined`.
- Cada evento mostra link do Meet/Zoom (`hangoutLink`/`conferenceData`): `o` abre no navegador e `y` copia o link via OSC 52.

### 3.3 Sistema (htop-lite) + detector de travamento
- **Métricas:** CPU total e por núcleo (sparkline de 60 s), load 1/5/15, memória, swap, discos (`/` e `/mnt/c`), rede ↓↑ em bytes/s, ping com jitter, uptime, número de processos e bateria, se `/sys/class/power_supply` existir.
- **Top 5 processos** por CPU e por memória, com os filhos agregados por nome (vários `node` viram uma linha só, com contagem).
- **Regras de perigo** (com histerese, para não piscar à toa):

| Regra | Aviso (`Warn`) | Crítico (`Critical`) |
|---|---|---|
| Memória usada | > 85 % | > 93 % |
| Swap | > 50 % e subindo | > 80 % |
| PSI memory some avg10 (melhor preditor de travamento) | > 5 | > 20 |
| CPU sustentada 10 s | > 90 % | > 98 % |
| Disco | > 90 % | > 97 % |

- **Visual do perigo:**
  - `Warn` pinta de amarelo.
  - `Critical` faz a métrica e os **maiores ofensores** piscarem em vermelho, e a borda do painel fica vermelha.
  - `K` sobre um processo manda SIGTERM, sempre depois de pedir confirmação.
- **WSL:** o `/proc` mostra a VM, e é isso que trava o seu shell, então vale como fonte principal. O provider opcional `wsl-host` mantém **um** `powershell.exe -NoProfile` persistente que imprime JSON a cada 3 s com `Get-NetAdapterStatistics` e `netsh wlan show interfaces` (SSID, sinal e taxa do **Wi-Fi real do Windows**). Fica desligado fora do WSL.
- **Velocidade:** `s` roda um *speedtest* sob demanda (download e upload via `speed.cloudflare.com/__down` e `__up`). Não é periódico, para não queimar banda.

### 3.4 Worktrees ("grove")
- O config define `workspaces = ["~/v360", "~/Workspace"]`. Cada um é varrido até profundidade 2 atrás de repositórios git, e cada repo roda `git worktree list --porcelain`.
- **Por worktree:** branch, número de arquivos sujos (`git status --porcelain=v2`), ↑↓ em relação ao upstream, idade do último commit, tamanho em disco (calculado com preguiça) e o **status das sessões Claude cujo `cwd` está dentro dela** (`◆` esperando você, `⠿` trabalhando, `●` parada).
- O cabeçalho agrega os totais: `N worktrees · ◆ esperando · ⠿ trabalhando · espaço livre`.
- **Seleção:** `↑↓`/`jk` navega, `/` abre filtro fuzzy (`nucleo-matcher`), `Enter` sai e dá `cd`, e `C` sai, dá `cd` e roda `claude --continue`.

### 3.5 Conversas do Claude em aberto
- **Fonte:** `~/.claude/sessions/*.json`, lidos com serde tolerante (todos os campos `Option` e `#[serde(default)]`).
- **Vivacidade:** `/proc/<pid>` precisa existir e o `procStart` precisa bater com o campo 22 de `/proc/<pid>/stat`. Isso descarta PID reciclado e arquivo órfão.
- **Enriquecimento:**
  - O título sai de `ai-title`/`agent-name`, lidos só no início do `.jsonl`.
  - O último `away_summary` e o horário da última atividade saem de um seek nos últimos 64 KB do arquivo, sem ler tudo.
- **Ordem:** `waiting` primeiro (destacado, porque é o que pede você), depois `busy` e por fim `idle`, com a idade relativa ao lado. O `kind=bg` ganha um ícone próprio.
- **Ações:** `Enter` dá `cd` para o cwd da sessão e `r` dá `cd` e roda `claude --resume <sessionId>`.
- **Risco:** esses formatos são internos e não documentados. Tudo fica isolado em `providers/claude.rs` com fixtures de teste. Se o formato mudar, o painel degrada para "cwd + status" em vez de quebrar.

### 3.6 Temas e customização
- **Theme** é um conjunto de tokens semânticos: `bg`, `fg`, `muted`, `border`, `accent`, `highlight`, `ok`, `warn`, `alert`, `alert_fg`, `spark` e `gauge_low/mid/high`.
- **Embutidos** (TOML carregados com `include_str!`): `dracula`, `catppuccin-mocha`, `tokyo-night`, `nord`, `gruvbox-dark`, `one-dark`, `solarized-dark` e `terminal` (usa só as 16 cores ANSI e respeita o tema do terminal).
- **Instaláveis:** arquivos em `~/.config/cockpit/themes/*.toml`. O `cockpit theme install <url|arquivo>` valida e copia o arquivo, e o `cockpit theme list` mostra os instalados. A tecla `t` troca o tema ao vivo.
- **Cores:** truecolor quando `COLORTERM=truecolor`. Sem isso, cada cor é convertida para a mais próxima na paleta de 256.
- **`~/.config/cockpit/config.toml`**, recarregado a quente via `notify`: tema, painéis visíveis, ordem e proporções, formato do relógio (12/24 h, segundos), relógios extras, limites de alerta e de perigo, cadências, workspaces, agendas incluídas ou excluídas, `city` fixa opcional, clima ligado ou desligado e provider WSL.

### 3.7 Integração com o shell (`cd`)
`cockpit shell-init zsh` imprime a função abaixo, que você coloca no `.zshrc` com `eval "$(cockpit shell-init zsh)"`:
```zsh
cockpit() {
  local tmp; tmp="$(mktemp -t cockpit.XXXXXX)"
  command cockpit --cd-file "$tmp" "$@"
  local dir cmd; { read -r dir; read -r cmd; } < "$tmp"; rm -f -- "$tmp"
  [[ -n $dir && -d $dir ]] && cd -- "$dir"
  case $cmd in
    continue)          claude --continue ;;
    resume:[0-9a-f-]*) claude --resume "${cmd#resume:}" ;;
  esac
}
```
Só existe uma lista fechada de comandos e não há `eval` de conteúdo arbitrário. Há também `--cd-file` para bash e fish.

## 4. Estrutura do projeto

```
~/copilot/
├── PLAN.md
├── Cargo.toml
├── src/
│   ├── main.rs            # clap: (run) | auth google | theme list/install | doctor | shell-init
│   ├── config.rs          # serde + defaults + hot-reload
│   ├── app/{state,action,update,event_loop}.rs
│   ├── alert.rs           # máquina de estados do alerta (Clock injetável)
│   ├── danger.rs          # regras de travamento + histerese
│   ├── providers/{mod,system,net,ping,psi,geo,weather,git,claude,wsl_host}.rs
│   ├── providers/calendar/{oauth,api,model}.rs
│   ├── theme/{mod,loader}.rs + theme/builtin/*.toml
│   ├── ui/{layout,hero,bigclock,map,agenda,system,worktrees,sessions,statusbar,blink}.rs
│   └── platform.rs        # detecção de WSL, open_url, OSC 52
└── tests/                 # fixtures: sessions/*.json, *.jsonl, git porcelain, respostas da API
```

**Crates** (versões fixadas no início da implementação, todas compatíveis com Rust 1.88):
- `ratatui` e `crossterm` (feature `event-stream`), para a interface.
- `tokio` (rt-multi-thread, time, sync, process, fs), para o runtime.
- `sysinfo`, para as métricas da máquina.
- `reqwest` (rustls, json), `oauth2` e `url`, para Calendar, geo e clima.
- `serde`, `toml`, `serde_json`, `chrono` e `chrono-tz`, para config, dados e fuso.
- `notify` e `notify-debouncer-mini`, para inotify e hot-reload.
- `nucleo-matcher` (filtro fuzzy), `walkdir` (tamanhos), `directories` (caminhos XDG) e `clap` (CLI).
- `anyhow` e `thiserror`, para erros.
- `tracing` e `tracing-appender`, com log **só em arquivo** (`~/.local/state/cockpit/cockpit.log`), nunca na tela.
- Opcional: `surge-ping`.

Não entram: `git2`/`gix` (spawn de `git` com cache é suficiente e mais leve), `google-calendar3` (pesada demais) e `notify-rust` (não funciona no WSL).

## 5. Roadmap de implementação

| Fase | Entrega | Pronto quando |
|---|---|---|
| F0 | Esqueleto: loop Elm, config, temas embutidos, layout responsivo, relógio grande, geo/cidade/fuso, relógios extras | `cockpit` abre, troca tema com `t`, CPU < 1 % |
| F1 | Sistema: métricas, top processos, PSI, discos, rede, ping, detector de perigo com blink | `stress-ng --vm` dispara Critical e o ofensor pisca |
| F2 | Worktrees + sessões Claude + seletor + `shell-init` + `cd`/`resume` | lista as 3 worktrees do `~/v360` e as sessões vivas, e `Enter` dá `cd` |
| F3 | `auth google`, provider de Calendar, painel de agenda, máquina de alerta vermelha, soneca | evento de teste daqui a 4 min deixa o hero vermelho, e `z` silencia |
| F4 | Provider WSL (Wi-Fi do host), speedtest `s`, clima, mapa braille | SSID e sinal do Windows aparecem, e `s` mede ↓↑ |
| F5 | `theme install/list`, hot-reload, `doctor`, polimento, `cargo install --path .` | tema instalado por URL, config recarregado sem reiniciar |

## 6. Verificação

- **Unitários:**
  - `alert.rs`, com hora falsa: Calm → Soon → Imminent, mais soneca, dispensa e reunião já iniciada.
  - `danger.rs`: limiares e histerese.
  - Parsers do Claude (sessions e jsonl), com fixtures reais anonimizadas.
  - Parser do `git worktree --porcelain`, leitura de temas e conversão truecolor → 256.
- **UI:** snapshots com `ratatui::backend::TestBackend` e `insta` para os tamanhos 200×50, 120×40 e 80×24, em estado normal e em estado de alerta.
- **Integração manual:**
  - `cockpit doctor` checa cada provider (token Google, git, `~/.claude`, PSI, powershell).
  - Um evento criado no Calendar para daqui a 4 min precisa deixar o hero vermelho.
  - `stress-ng --vm 2 --vm-bytes 90%` precisa fazer os ofensores piscarem.
  - O seletor precisa dar `cd` de verdade pelo wrapper.
- **Performance:** `pidstat -p $(pgrep cockpit) 1 60` deve ficar abaixo de 1 % de CPU com o painel parado, e `ps -o rss` abaixo de 40 MB. O modo `--bench-frames` mede o tempo por frame.
