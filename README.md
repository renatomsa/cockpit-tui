# cockpit-tui

Cockpit de terminal para quem passa o dia no shell. Numa tela só:
- relógio grande com cidade, clima e mapa;
- próxima reunião do Google Calendar, com alerta vermelho antes de começar;
- chats do Claude Code embutidos, em abas, rodando dentro do cockpit;
- saúde da máquina: CPU e memória, com aviso quando ameaça travar;
- seletor de pasta para o chat novo, com os repositórios e worktrees;
- conversas do Claude Code, com tokens por rodada e limites do plano.

Escrito em Rust. Parado, usa menos de 1 % de CPU e cerca de 30 MB de memória.

## Instalar

```sh
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --git ssh://git@github.com/renatomsa/cockpit-tui.git
cockpit config init                                    # ~/.config/cockpit/config.toml comentado
cockpit
```

O repositório é privado. A variável `CARGO_NET_GIT_FETCH_WITH_CLI` faz o cargo usar o `git` do sistema, com a sua chave SSH do GitHub. Para não repeti-la, ponha `[net]` com `git-fetch-with-cli = true` em `~/.cargo/config.toml`. Para usar HTTPS em vez de SSH, rode `gh auth setup-git` uma vez e use `--git https://github.com/renatomsa/cockpit-tui`.

- **Atualizar:** o mesmo comando com `--force`.
- **A partir de um clone:** `cargo install --path .`.
- **Ver o que falta configurar:** `cockpit doctor`.

## Painéis

| Painel | O que mostra |
|---|---|
| **Relógio** | no alto à direita: cidade (pelo IP ou fixa no config) e relógios extras; próxima reunião; clima e próximas horas (Open-Meteo) |
| **Agenda** | reuniões das próximas 48 h, com link da chamada |
| **Chat** | o `claude` rodando dentro do cockpit, um por aba; é o painel grande, no alto à esquerda, com a agenda embaixo |
| **Sistema** | abaixo do relógio: caixas de CPU e Memória com detalhe por tipo de uso; maiores ofensores de CPU e memória e, logo abaixo, os limites de 5 h e da semana do Claude |

Alertas:
- **Reunião:** a borda fica amarela a 15 min. De 5 min antes a 5 min depois do início, o relógio inteiro fica vermelho e piscando.
- **Sistema:** amarelo no aviso e vermelho em negrito no crítico, para memória, swap, pressão de memória (PSI), CPU sustentada e disco. Os limites ficam em `[system.danger]`.

A tecla `i` mostra a legenda dos símbolos do painel em foco.

## Teclas

| Onde | Tecla | Ação |
|---|---|---|
| geral | `Tab` / `Shift+Tab` | troca o painel em foco |
| | `t` / `T` | próximo / anterior tema |
| | `v` · `Esc` | ditado: grava; `v` de novo transcreve e escreve no chat, sem Enter · descarta |
| | `i` · `?` · `x` | legenda · ajuda · dispensa o aviso da barra |
| | `q` / `Ctrl+C` | sai (com chats abertos, pede confirmação: sair encerra todos) |
| alerta de reunião | qualquer tecla · `z` · `o` | para o alerta · adia 5 min · abre a chamada |
| Agenda | `o` · `y` | abre a chamada · copia o link |
| Chat | `n` · `Enter` | chat novo (escolha a pasta: digitar filtra, Enter abre) · digita no chat |
| | `← →` · `1`-`9` · `x` | troca de chat · vai direto · fecha (encerra o `claude` dele) |
| digitando no chat | `Ctrl+Q` | para de digitar: as teclas voltam ao cockpit (todas as outras, inclusive `Tab`, `Esc` e `Ctrl+C`, vão para o `claude`) |
| | `Alt+V` · `Esc` | ditado, como o `v` · descarta (o `Esc` só fica com o cockpit enquanto o ditado está ligado) |
| | `Alt+1`-`9` · roda do mouse ou `Shift+PgUp/PgDn` | troca de chat · rola o histórico (Shift+arrastar seleciona texto do cockpit) |
| qualquer painel | `a` | permite desta vez o pedido de permissão, quando há um só pendente |
| Sistema | `← →` `↑↓` · `K` | escolhe processo · encerra com SIGTERM, com confirmação |

## Comandos

| Comando | O que faz |
|---|---|
| `cockpit doctor` | confere cada peça e diz como resolver o que falta |
| `cockpit theme list` / `install <arquivo ou URL>` / `remove <nome>` | gerencia temas |
| `cockpit config init` / `config path` | cria o config / mostra onde ficam config, temas, estado e log |
| `cockpit calendar add <endereço iCal>` / `list` / `remove <n>` | conecta a agenda pelo endereço iCal |
| `cockpit auth google` | conecta o Google Calendar por OAuth (alternativa) |
| `cockpit claude-hook install` / `uninstall` / `status` | liga o "permitir desta vez" pelo cockpit |
| `cockpit --demo-agenda` | reuniões de exemplo, para ver o alerta sem o Google |

## Permitir desta vez

Com o hook instalado (`cockpit claude-hook install`, que acrescenta um hook `PermissionRequest` ao `~/.claude/settings.json` e guarda um backup), as conversas do Claude Code que pedirem permissão aparecem no cockpit com o que vão fazer, por exemplo `◆ Bash: git push origin main`. O `a` permite **só aquela vez**.

O diálogo continua aparecendo no terminal, e quem responder primeiro vale. Se você responder lá, o hook percebe e sai sozinho. Cada pedido tem um código aleatório, e só uma resposta com esse código vale; os pedidos ficam numa pasta privada (`~/.local/state/cockpit/permissions`, 0700). O hook vale para as sessões abertas depois da instalação.

## Configuração

Tudo fica em `~/.config/cockpit/config.toml`, cujo modelo está em [`config.example.toml`](config.example.toml). O arquivo é **recarregado ao salvar**:
- **Na hora:** tema, relógio, layout, idioma e tempos do alerta.
- **Ao reabrir:** as seções lidas na abertura. Quando você muda uma delas, o cockpit avisa.
- **Com erro:** o config não é aplicado, e o cockpit mantém o anterior.

Ajustes mais comuns:

```toml
theme = "dracula"                 # veja a lista abaixo, ou `cockpit theme list`

[location]                        # o IP costuma errar a cidade
city = "Rio de Janeiro"
timezone = "America/Sao_Paulo"

[[clock.extra]]
label = "Orlando"
tz = "America/New_York"

[worktrees]
workspaces = ["~"]                # onde procurar repositórios (as opções do chat novo)
```

**Temas embutidos** (`t` percorre todos, na ordem):

| Estilo | Temas |
|---|---|
| Escuros | `dracula` `catppuccin-mocha` `tokyo-night` `nord` `gruvbox-dark` `one-dark` `solarized-dark` `rose-pine` `kanagawa` `everforest` `ayu-dark` `night-owl` `material` |
| JetBrains | `darcula` (clássico) `jetbrains-dark` (interface nova) `intellij-light` |
| Monokai | `monokai` `monokai-pro` |
| Editores | `github-dark` `github-light` `vscode-dark` |
| Claros | `intellij-light` `github-light` `catppuccin-latte` `solarized-light` |
| Terminal | `terminal` (só as 16 cores ANSI, segue o tema do seu terminal) |

Todos passam num teste de contraste (WCAG): texto sobre fundo ≥ 4,5 e destaques e alertas ≥ 3.

**Temas próprios** são arquivos `.toml` com tokens de cor (`bg fg muted border accent highlight ok warn alert alert_bg alert_fg spark gauge_low gauge_mid gauge_high clock`). O que faltar herda de outro token. `cockpit theme install` valida o tema antes de instalar, e um cockpit aberto já o mostra no ciclo do `t`.

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

## Ditado

`v` (ou `Alt+V` digitando no chat) começa a gravar o microfone; a barra mostra `● gravando 0:07`. De novo, a gravação para, é transcrita e o texto entra no chat selecionado sem Enter, para você revisar antes de mandar. Sem chat aberto, vai para a área de transferência. `Esc` descarta.

Tudo roda local: um gravador do sistema (`pw-record`, `parecord`, `arecord` ou o `rec` do sox) e o [whisper.cpp](https://github.com/ggml-org/whisper.cpp). Para instalar o whisper.cpp no caminho que o config padrão usa:

```sh
git clone --depth 1 https://github.com/ggml-org/whisper.cpp ~/.local/share/whisper.cpp
cd ~/.local/share/whisper.cpp
cmake -B build -DCMAKE_BUILD_TYPE=Release && cmake --build build -j --target whisper-cli
ln -sf "$PWD/build/bin/whisper-cli" ~/.local/bin/whisper-cli
curl -L -o models/ggml-small.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
cockpit doctor     # confere gravador, transcritor e modelo
```

Sem `cmake`: `uv tool install cmake`. O modelo `small` (466 MB) é o menor que acerta bem em português; numa CPU de notebook, uma frase leva uns 3 a 4 s. Para trocar o transcritor (outro modelo, outra ferramenta), ajuste `[voice].transcribe`: qualquer comando que leia o WAV e imprima o texto serve.

## Privacidade e custo

- **Limites do plano:** o cockpit lê o login do Claude Code em `~/.claude/.credentials.json` e consulta `api.anthropic.com` a cada 3 min. O token nunca é renovado nem gravado. Para desligar: `[claude].usage_limits = false`.
- **Rede:** a cidade vem de ipapi.co ou ipinfo.io, e o clima, da Open-Meteo.
- **Ditado:** o áudio é gravado num arquivo temporário, transcrito na máquina e apagado em seguida.
- **Git:** a lista de worktrees (`git worktree list`) é relida a cada 60 s ou quando uma worktree é criada ou removida.

## Desenvolvimento

```sh
cargo test
cargo clippy --all-targets
COCKPIT_LOG=debug cargo run      # log em ~/.local/state/cockpit/cockpit.log
```
