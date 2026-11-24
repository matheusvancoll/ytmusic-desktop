<div align="center">

# YT Music

**Um player de YouTube Music leve para Windows. Sem navegador, sem Electron.**

![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust&logoColor=white)
![Windows](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4?logo=windows&logoColor=white)
![egui](https://img.shields.io/badge/UI-egui-blueviolet)
![Estado](https://img.shields.io/badge/estado-em%20desenvolvimento-yellow)

</div>

---

Um app nativo, em um único `.exe` de poucos MB, com o visual do YouTube Music e a sua conta conectada: busca, músicas curtidas, playlists e uma fila infinita, tudo sem abrir uma aba do navegador.

<div align="center">

### [⬇️ Baixar o instalador para Windows](../../releases/latest)

`YTMusic-Setup-<versão>.exe` · instala só para o seu usuário, sem precisar de administrador

<br>

<img src="docs/screenshot.png" alt="YT Music: lista de músicas curtidas, com a barra do player embaixo" width="860">

</div>

## Baixar e instalar

1. Abra a página de **[Releases](../../releases/latest)** e baixe o arquivo **`YTMusic-Setup-<versão>.exe`** da versão mais recente.
2. Execute o instalador. Ele instala em `%LOCALAPPDATA%\Programs\YT Music`, cria o atalho no Menu Iniciar (e na Área de Trabalho, se você marcar) e oferece instalar o **yt-dlp** e o **Deno** pelo `winget`. O yt-dlp é obrigatório para tocar músicas; o Deno é o runtime JavaScript que ele usa para o YouTube.
3. Abra o **YT Music**, clique no avatar (canto superior direito) e conecte sua conta. Veja [Conectando sua conta](#conectando-sua-conta).

**Confira o download (opcional).** O hash SHA-256 do instalador está nas notas de cada Release. Para comparar:

```powershell
Get-FileHash .\YTMusic-Setup-<versão>.exe -Algorithm SHA256
```

> O instalador não tem assinatura digital. Por isso o Windows (SmartScreen) ou o antivírus podem avisar na primeira execução; é um falso positivo comum em programas novos. No SmartScreen, clique em **Mais informações** e depois em **Executar assim mesmo**.

**Desinstalar:** Configurações do Windows → Aplicativos → **YT Music**. A desinstalação também apaga `%LOCALAPPDATA%\ytmusic` (sessão, volume e cache).

**Atualizar:** abra o app, vá em **Configurações → Atualizações** e clique em **Verificar atualizações**. O app não verifica sozinho: só consulta o GitHub quando você pede e só instala se você aceitar. Também dá para baixar o instalador novo e executá-lo por cima da versão atual.

## Recursos

- **Visual do YouTube Music**: barra superior com busca, menu lateral, lista de músicas com álbum e duração, grade de playlists e barra do player com progresso vermelho.
- **Sua conta**: curtidas e playlists da biblioteca, depois de um login único.
- **Mix infinito**: toque em uma música e a fila continua sozinha com faixas parecidas.
- **Controles completos**: anterior, play/pause, próxima, buscar no tempo, volume (lembrado entre sessões), repetir (fila ou faixa), aleatório e silenciar.
- **Responsivo**: em janelas estreitas o menu lateral vira abas e o player se reorganiza em duas linhas.
- **Leve**: interface em [egui](https://github.com/emilk/egui) com renderização por GPU, áudio decodificado em Rust puro. Sem `mpv` nem `ffmpeg`.
- **Próxima faixa já pronta**: o app baixa a próxima música da fila enquanto a atual toca, então a troca é imediata.
- **Teclas de mídia**: play/pause, próxima e anterior do teclado e do fone, com o painel de mídia do Windows (capa e título), mesmo minimizado.
- **Mini-player flutuante**: pelo botão de mini-player na barra superior, a janela principal é minimizada e um mini-player de vidro fica sobre todas as janelas (minimizar pelo Windows continua só minimizando). Dois formatos (completo, com aleatório/repetir/Mix, e compacto, só com os controles), arrastável, com botão de travar, esconder e fechar que aparecem no hover.
- **A seguir / Relacionadas**: painel à direita com a fila e as sugestões da música atual.
- **Playlists no menu**: clique em Playlists para listar todas no menu lateral; arraste uma música para uma delas para adicioná-la.
- **Atalhos de teclado**: Espaço, ←/→, Ctrl+←/→, ↑/↓, M, S, R, Ctrl+F (lista completa em Configurações > Geral).
- **Português e inglês**: textos por chave em `locales/*.json` (segue o idioma do Windows; troque em Configurações > Geral).
- **Automix** (desligado por padrão): mescla o fim de uma música com o início da próxima, com duração ajustável de 2 a 12 segundos. Só acontece quando a próxima já foi baixada e não vale com *repetir faixa*.
- **Controles na barra de tarefas**: passe o mouse sobre o ícone do app e use anterior, play/pause e próxima direto na miniatura.
- **Configurações**: janela própria com as páginas Conta, Reprodução, Atualizações e Geral.
- **Atualizações sob demanda**: verificação manual pelo app, com o download conferido por SHA-256.

## Como o app é organizado

```
┌────────────────────────────────────────────────────────────┐
│ ◉ Music        [ 🔍 Pesquisar músicas, artistas... ]    (M) │
├──────────────┬─────────────────────────────────────────────┤
│ 🔍 Buscar    │  Músicas que você curtiu        [▶ Reproduzir] │
│ ♥  Curtidas  │  ┌──┐ Título           Álbum            3:49 │
│ ≡  Playlists │  └──┘ Artista                                │
├──────────────┴─────────────────────────────────────────────┤
│ ━━━━━━━━━●━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ │
│ ⏮  ▶  ⏭  0:12/3:45    ▢ Título · Artista • Álbum    🔊━ 🔁 🔀 Mix │
└────────────────────────────────────────────────────────────┘
```

## Compilar a partir do código

Para usar o app, prefira o [instalador](#baixar-e-instalar). Esta seção é para quem quer compilar ou desenvolver.

### Requisitos

- Windows 10 ou 11 com o **WebView2 Runtime** (já vem no Windows 11), usado só na janela de login.
- [Rust](https://rustup.rs/) 1.85 ou mais novo (edição 2024), para compilar.
- **yt-dlp**, que fornece o áudio:

  ```powershell
  winget install yt-dlp.yt-dlp
  ```

  Versões recentes do yt-dlp também precisam de um runtime JavaScript (**Deno** ou **Node**) para o YouTube. O app passa `--js-runtimes` automaticamente se achar um deles no `PATH` ou na pasta de pacotes do `winget`.

### Compilar e rodar

```powershell
git clone <url-do-repositorio>
cd ytmusic

cargo build --release   # gera ytmusic.exe e ytmusic-login.exe
cargo run --release     # abre o app
```

Os dois executáveis (`ytmusic.exe` e `ytmusic-login.exe`) precisam ficar **na mesma pasta**. Se mexer no login, rode `cargo build --release` de novo, porque o `cargo run` só recompila o app principal.

> O perfil `release` usa `lto` completo e `opt-level = "z"` para gerar um executável pequeno. Por isso o build de release demora alguns minutos; para iterar, use `cargo run` (debug).

### Gerar o instalador (`Setup.exe`)

Requisitos: Rust (com as *Build Tools do Visual Studio* / MSVC) e o [Inno Setup 6](https://jrsoftware.org/isinfo.php) (`winget install JRSoftware.InnoSetup`).

```powershell
powershell -File installer\build.ps1
```

O script compila o release e gera `installer\Output\YTMusic-Setup-<versão>.exe`, que instala o app por usuário (sem precisar de administrador) e, se você quiser, instala o yt-dlp e o Deno pelo winget.

**Privacidade do build:** o compilador costuma embutir no `.exe` caminhos da máquina que compilou (como `C:\Users\<você>\.cargo\...`). O script reescreve esses prefixos e, ao final, **aborta** se achar o usuário do Windows, a pasta do projeto ou o nome do computador dentro dos executáveis ou do instalador. Para uma distribuição pública, o instalador é gerado em um ambiente limpo, no CI do GitHub.

### Publicar uma versão

Os instaladores das [Releases](../../releases) são gerados pelo GitHub Actions (`.github/workflows/release.yml`), não na máquina de quem desenvolve:

1. Atualize `version` no `Cargo.toml` (por exemplo, `0.3.0`) e escreva as novidades em `release-notes/0.3.0.md` (o app as mostra na aba Atualizações). Faça o commit.
2. Crie e envie uma tag com o mesmo número, com o prefixo `v`: `v0.3.0`.
3. O workflow confere se a tag bate com a versão do `Cargo.toml`, compila, gera `YTMusic-Setup-<versão>.exe`, calcula o SHA-256 e publica o Release com o instalador, as novidades e o hash nas notas.

O atualizador do app procura justamente o arquivo `YTMusic-Setup-<versão>.exe` e o SHA-256 do Release mais recente.

> Antivírus podem bloquear o build ou o instalador resultante (falso positivo, comum em programas Rust e em instaladores sem assinatura digital).

## Conectando sua conta

Clique no avatar (canto superior direito) e em **Conectar conta do YouTube**. Uma janela de login do Google se abre; entre normalmente, com senha, verificação em duas etapas ou passkey, e ela fecha sozinha.

Se o Google bloquear o login na janela embutida, use **Importar cookie manualmente** no mesmo menu: cole o cabeçalho `Cookie` de uma requisição para `music.youtube.com` (DevTools, aba Rede).

## Privacidade e segurança

- O app **nunca vê sua senha**. O login acontece na página real do Google, dentro do WebView2, em um processo separado (`ytmusic-login.exe`) que só existe durante o login.
- A sessão do WebView2 é descartável (modo incógnito). Nada do login fica no perfil dele.
- O app guarda **somente os cookies da sessão**, criptografados com o **DPAPI do Windows**, em `%LOCALAPPDATA%\ytmusic\auth.bin`. Só o seu usuário do Windows consegue descriptografar.
- Nada é enviado para servidores de terceiros. O app conversa apenas com o YouTube e o Google (as capas vêm dos servidores de imagem deles) e, **somente quando você clica em "Verificar atualizações"**, com o GitHub (para ler o último Release e baixar o instalador). Essa consulta é um pedido HTTP comum, sem identificador seu.
- As atualizações **nunca são instaladas sozinhas**: o app só baixa o instalador depois que você aceita, e recusa o arquivo se o SHA-256 não bater com o publicado no Release.
- **Desconectar** apaga o `auth.bin`.

> O cookie da sessão dá acesso à sua conta Google, não só ao YouTube Music. Nunca compartilhe o `auth.bin` nem o cookie que você cola em "Importar manualmente".

## Onde ficam os dados

Tudo em `%LOCALAPPDATA%\ytmusic\`:

| Arquivo | Conteúdo |
| --- | --- |
| `auth.bin` | cookies da sessão, protegidos pelo DPAPI |
| `volume` | último volume usado |
| `prefs.json` | preferências (idioma, mini-player, painel) |
| `automix` | automix ligado/desligado e a duração da mescla (sem o arquivo, fica desligado) |
| `cache\` | áudios das últimas 80 faixas (cerca de 4 MB cada), limpo automaticamente |

Para remover tudo, apague essa pasta.

## Desenvolvimento

```powershell
cargo test                                         # testes unitários
cargo test live_search -- --ignored --nocapture    # chama a API de verdade
cargo run -- --search "nome da música"             # depuração: imprime os resultados da busca
```

Estrutura do código:

| Arquivo | Função |
| --- | --- |
| `src/app.rs` | interface (egui): layout, listas, player bar, fila |
| `src/app/settings.rs` | janela de configurações (Conta, Reprodução, Atualizações, Geral) |
| `src/app/update.rs` | estados e ações da atualização no app (sempre a pedido) |
| `src/updater.rs` | consulta os Releases do GitHub, baixa e confere o instalador (SHA-256) |
| `src/ytm.rs` | cliente da API interna do YouTube Music (busca, biblioteca, mix) |
| `src/player.rs` | reprodução: yt-dlp, cache, decodificação, automix e controle de áudio (rodio) |
| `src/taskbar.rs` | botões de controle na miniatura da barra de tarefas |
| `src/auth.rs` | credenciais: cookie, `SAPISIDHASH` e proteção via DPAPI |
| `src/bin/login.rs` | janela de login em WebView2 (executável separado) |
| `src/mini.rs` | mini-player flutuante (vidro, dois formatos) |
| `src/media.rs` | teclas de mídia e painel de mídia do Windows (SMTC) |
| `src/i18n.rs` | traduções por chave (`tr!("chave")`), arquivos em `locales/` |
| `src/prefs.rs` | preferências (`prefs.json`) |
| `installer/` | script do instalador (Inno Setup) e o `build.ps1` que o gera |

## Stack

[egui / eframe](https://github.com/emilk/egui) · [rodio](https://github.com/RustAudio/rodio) + [symphonia](https://github.com/pdeljanov/Symphonia) · [wry](https://github.com/tauri-apps/wry) + [tao](https://github.com/tauri-apps/tao) · [ureq](https://github.com/algesten/ureq) · [yt-dlp](https://github.com/yt-dlp/yt-dlp)

## Aviso

Este projeto é independente e **não é afiliado ao YouTube nem ao Google**. Ele usa a API interna do YouTube Music (a mesma do site), que não é oficial e pode mudar sem aviso. Feito para uso pessoal; respeite os Termos de Serviço do YouTube.
