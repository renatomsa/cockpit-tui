# copilot-tui · `cockpit`

Cockpit de terminal para quem passa o dia no shell: relógio grande com a cidade atual, agenda do Google Calendar com alerta vermelho antes da reunião, um htop enxuto que pisca quando a máquina ameaça travar, worktrees do Claude com seletor que dá `cd` e as conversas do Claude em aberto.

A arquitetura e o roadmap estão em [`PLAN.md`](PLAN.md).

## Estado

| Fase | Entrega | Status |
|---|---|---|
| F0 | Loop, config, temas, layout responsivo, relógio grande, cidade/fuso por IP, relógios extras | ✅ |
| F1 | Sistema (CPU, memória, swap, disco, rede, ping, PSI) + detector de travamento | ✅ |
| F2 | Worktrees + sessões do Claude + seletor com `cd` | ✅ |
| F3 | Google Calendar (OAuth2) + alerta vermelho | ✅ |
| F4 | Wi-Fi do host (WSL), speedtest, clima, mapa | ✅ |
| F5 | `theme install`, hot-reload, `doctor` | ✅ |

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
| `← →` (Worktrees/Claude) | troca entre as abas Worktrees e Claude |
| `↑↓` `j k` `g G` (Worktrees) | escolhe a worktree |
| `D` (Worktrees) | exclui a worktree: `y` no primeiro aviso, Enter no segundo |
| `Enter` (Worktrees) | sai do cockpit e vai para a pasta da worktree |
| `C` (Worktrees) | abre `claude --continue` numa aba nova, já na worktree; o cockpit continua aberto |
| `/` (Worktrees) | filtra por nome ou branch (`Enter` aplica, `Esc` limpa) |
| `Enter` (Claude) | sai e vai para a pasta da conversa |
| `r` (Claude) | retoma uma conversa encerrada (`claude --resume`) numa aba nova |
| `i` | legenda dos símbolos do painel em foco |
| qualquer tecla (alerta vermelho) | para o alerta da reunião |
| `z` (alerta vermelho) | adia o alerta por 5 min |
| `o` (alerta vermelho ou Agenda) | abre a chamada (Meet, Zoom, Teams) no navegador |
| `y` (Agenda) | copia o link da reunião |
| `K` (Docker) | para o container selecionado (`docker stop`), com confirmação |
| `X` (Docker) | derruba tudo: `docker compose down --remove-orphans` em cada projeto e `docker stop` nos soltos, com confirmação |
| `←→` `↑↓` (painel Sistema) | escolhe a coluna (TOP CPU / TOP MEM) e o processo |
| `s` (painel Sistema) | teste de velocidade (download e upload) |
| `K` (painel Sistema) | encerra o processo com SIGTERM, depois de pedir confirmação |
| `?` | ajuda |
| `q` / `Ctrl+C` | sai |

Outros comandos:

| Comando | O que faz |
|---|---|
| `cockpit doctor` | confere cada peça (config, temas, shell, git, Claude, Docker, Google, rede, WSL) e diz como resolver o que falta |
| `cockpit theme list` | temas embutidos e instalados |
| `cockpit theme install <arquivo ou URL>` | valida e instala um tema (links do GitHub valem); `--force` substitui |
| `cockpit theme remove <nome>` | remove um tema instalado |
| `cockpit config init` / `config path` | cria o config comentado / mostra onde ficam config, temas, estado e log |
| `cockpit auth google` | conecta o Google Calendar |
| `cockpit shell-init zsh\|bash\|fish` | função que faz o Enter trocar de pasta |
| `cockpit --demo-agenda` | reuniões de exemplo, para ver o alerta sem o Google |
| `cockpit --theme nord` | tema só nesta execução |

O config e os temas são **recarregados sozinhos** quando você salva o arquivo. Tema, relógio, layout, idioma e tempos do alerta mudam na hora. As seções lidas na abertura (`[system]`, `[worktrees]`, `[location]`, `[network]`, `[claude]`, `[weather]` e a busca do `[calendar]`) mostram um aviso de que valem ao reabrir. Um config com erro não é aplicado: o cockpit mantém o anterior e mostra a linha do problema.

## Temas

Embutidos: `dracula`, `catppuccin-mocha`, `tokyo-night`, `nord`, `gruvbox-dark`, `one-dark`, `solarized-dark` e `terminal` (só cores ANSI, segue o tema do seu terminal).

Para instalar um tema, use `cockpit theme install <arquivo ou URL>`, que valida as cores antes de copiar, ou coloque um `.toml` em `~/.config/cockpit/themes/`. Num cockpit aberto, ele aparece na hora no ciclo do `t`. Tokens ausentes herdam de outros, e um arquivo com o mesmo `name` de um embutido o substitui:

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

No topo do painel Sistema ficam duas caixas:
- **CPU:** barra empilhada por tipo de uso (usuário, sistema, nice, espera de E/S, steal) com legenda, tempo ocioso, temperatura quando há sensor (no WSL não há), carga de 1, 5 e 15 min com o percentual sobre os núcleos, e o total de processos e threads.
- **Memória:** barra empilhada com apps (memória dos programas), kernel, compartilhada, comprimida (zswap) e cache de arquivos, o quanto está em uso, a pressão (baixa, média ou alta, pelo PSI), livre, disponível e swap.
- **Rede:** tráfego agora, Wi-Fi (rede, sinal, padrão, canal, faixa e link), ping e o último teste de velocidade. No WSL, o tráfego e o Wi-Fi são os do Windows, lidos por um único PowerShell que fica aberto em segundo plano (~0,5 % de um núcleo e ~80 MB no Windows; desligue com `[network].wsl_host = false`). O `s` roda o teste de velocidade contra a Cloudflare, baixando 25 MB e enviando 10 MB, só quando você pede.
- **Disco:** uma barra por volume (`/`, `/mnt/c`…), com percentual e espaço livre, e a leitura e a escrita em disco do momento, somando os discos do Linux.

Embaixo das caixas, o painel mostra pressão (PSI), discos, rede da interface da rota padrão, ping e os maiores ofensores de CPU e memória, agrupados por nome.

| Métrica | Amarelo | Vermelho em negrito |
|---|---|---|
| Memória | > 85 % | > 93 % |
| Swap (o amarelo só entra se ela estiver subindo) | > 50 % | > 80 % |
| PSI memória (% do tempo com tarefas travadas esperando RAM) | > 5 | > 20 |
| CPU média de 10 s | > 90 % | > 98 % |
| Disco | > 90 % | > 97 % |

No vermelho, a métrica e os maiores ofensores ficam em vermelho negrito (sem piscar: o pisca é só do alerta de reunião), a borda do painel fica vermelha e o motivo aparece também na barra de status, que continua visível no modo compacto. Para baixar, o valor precisa cair 3 pontos abaixo do limiar, o que evita pisca-pisca. Tudo isso é configurável em `[system.danger]`.

No rodapé do painel fica o bloco MÁQUINA: sistema operacional, processador, núcleos físicos e threads, placa de vídeo e memória. No WSL, o Linux só enxerga a VM. Por isso os números vêm da máquina real, lidos uma vez do Windows pelo PowerShell na abertura, e o que o WSL recebe aparece entre parênteses, como em `8 threads (WSL usa 6)`. No Linux nativo, a placa de vídeo vem do `nvidia-smi` ou do `lspci`.

O ping mede o tempo de conexão TCP a `1.1.1.1:443`, porque o WSL vem com ICMP sem root desligado (`net.ipv4.ping_group_range = 1 0`).

## Google Calendar

Com a agenda conectada, o painel Agenda lista as reuniões das próximas 48 h de todas as agendas visíveis no seu Google Calendar, sem as que você recusou. O relógio mostra a próxima reunião:
- **Faltando até 15 min:** a borda do relógio e a linha da reunião ficam amarelas.
- **Faltando até 5 min, ou até 5 min depois de começar:** o relógio inteiro fica vermelho e a linha da reunião pisca. Qualquer tecla para o alerta daquela reunião, `z` adia por 5 min e `o` abre a chamada.

Eventos de dia inteiro nunca alertam. Os tempos são configuráveis em `[calendar]`.

Para ver o alerta sem conectar nada: `cockpit --demo-agenda`, que cria uma reunião de exemplo começando em 4 min.

### Conectar (uma vez)

O cockpit usa um cliente OAuth seu, e não um app de terceiros; o acesso é somente leitura (`calendar.readonly`).

1. Em [console.cloud.google.com](https://console.cloud.google.com), crie um projeto (ex.: `cockpit`).
2. Em **APIs e serviços → Biblioteca**, ative a **Google Calendar API**.
3. Em **Google Auth Platform → Público-alvo** (tela de consentimento), escolha **Interno** se a sua conta for do Google Workspace da empresa. Com conta pessoal, escolha **Externo** e adicione o seu e-mail como usuário de teste.
4. Em **Google Auth Platform → Clientes → Criar cliente**, escolha o tipo **App para computador**, dê um nome e baixe o JSON.
5. Salve o JSON onde o cockpit procura. No WSL, a pasta Downloads do Windows fica em `/mnt/c/Users/<você>/Downloads`:
   ```sh
   mkdir -p ~/.config/cockpit
   cp /mnt/c/Users/<você>/Downloads/client_secret_*.json ~/.config/cockpit/google_client.json
   ```
6. Rode `cockpit auth google`. O navegador abre, você autoriza e a página confirma "Agenda conectada". Um cockpit já aberto pega a agenda sozinho em até 15 s.

O token fica em `~/.local/state/cockpit/google_token.json`, com permissão 0600, e é renovado sozinho. Para desconectar, apague esse arquivo e remova o acesso em [myaccount.google.com/permissions](https://myaccount.google.com/permissions). Se a empresa bloquear apps não verificados, o administrador do Workspace precisa liberar o projeto.

## Docker

O painel Docker, entre a Agenda e as Worktrees, lista os containers rodando: o equivalente ao `docker ps`, sem `-a`. Mostra nome, imagem, há quanto tempo está de pé, as portas publicadas e o projeto compose. A cor da bolinha é o healthcheck: verde saudável, azul sem healthcheck, amarelo iniciando, vermelho falhando. A lista vem direto do socket do daemon a cada 3 s, o que custa cerca de 1 ms, sem rodar a CLI.

- `K` para o container selecionado com `docker stop`, que pode ser religado depois com `docker start`.
- `X` derruba tudo. Para cada projeto compose roda `docker compose -p <projeto> --project-directory <pasta> -f <arquivos> down --remove-orphans`, com pasta e arquivos lidos das labels que o compose gravou; se eles não existem mais, roda só com `-p`. Containers fora de compose recebem `docker stop`. Volumes não são apagados.

As duas teclas mostram antes o que vai rodar e só executam com `y`.

## Worktrees e conversas do Claude

Worktrees e Claude ficam no mesmo painel, em duas abas trocadas com `← →`. O título mostra quantas worktrees e conversas há, o aviso de conversas esperando você e o resumo dos limites do plano (`5h 43% · semana 28%`), que fica à vista nas duas abas.

A aba Worktrees varre as pastas de `[worktrees].workspaces` (padrão: `~`, dois níveis) atrás de repositórios git e lista as worktrees de cada um: branch, arquivos alterados (`✎`), commits à frente e atrás do upstream (`↑↓`), idade do último commit, tamanho em disco e as conversas do Claude abertas nela (`◆` esperando você, `⠿` trabalhando, `$` rodando shell, `●` parada).

O `D` exclui a worktree selecionada com `git worktree remove`, em dois passos. O primeiro aviso mostra a pasta e a branch e continua com `y`. O segundo diz o que se perde, como os arquivos sem commit e as conversas do Claude abertas nela, e só apaga com Enter, para que dois `y` seguidos não apaguem nada por engano. Com alterações pendentes, o comando leva `--force`. A branch continua existindo, e a pasta principal do repositório não pode ser excluída por aqui.

A aba Claude lista as conversas abertas (lidas de `~/.claude/sessions`), com as que esperam você primeiro, e as encerradas nas últimas 24 h, que o `r` retoma. Cada conversa entra na worktree em que o Claude está de fato trabalhando, lida da transcrição, mesmo que ele tenha sido aberto na raiz do repositório.

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

## Clima e mapa

Abaixo da reunião, o relógio mostra o clima da cidade atual pela [Open-Meteo](https://open-meteo.com) (sem chave, a cada 15 min): a condição agora, a temperatura, a hora com mais chance de chuva nas próximas 12 h e a previsão das próximas horas. À direita do relógio, quando há largura, fica um mapa-múndi com a cidade marcada. Desligue com `[weather].enabled = false` e `[clock].map = false`. Em telas baixas, o clima some antes do relógio encolher.

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
