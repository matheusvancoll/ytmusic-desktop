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

## Recursos

- **Visual do YouTube Music**: barra superior com busca, menu lateral, lista de músicas com álbum e duração, grade de playlists e barra do player com progresso vermelho.
- **Sua conta**: curtidas e playlists da biblioteca, depois de um login único.
- **Mix infinito**: toque em uma música e a fila continua sozinha com faixas parecidas.
- **Controles completos**: anterior, play/pause, próxima, buscar no tempo, volume (lembrado entre sessões), repetir (fila ou faixa), aleatório e silenciar.
- **Responsivo**: em janelas estreitas o menu lateral vira abas e o player se reorganiza em duas linhas.
- **Leve**: interface em [egui](https://github.com/emilk/egui) com renderização por GPU, áudio decodificado em Rust puro. Sem `mpv` nem `ffmpeg`.
- **Próxima faixa já pronta**: o app baixa a próxima música da fila enquanto a atual toca, então a troca é imediata.

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

## Instalação

### Requisitos

- Windows 10 ou 11 com o **WebView2 Runtime** (já vem no Windows 11), usado só na janela de login.
- [Rust](https://rustup.rs/) 1.85 ou mais novo (edição 2024), para compilar.
- **yt-dlp**, que fornece o áudio:

  ```powershell
  winget install yt-dlp.yt-dlp
  ```

  Versões recentes do yt-dlp também precisam de um runtime JavaScript (**Deno** ou **Node**) para o YouTube. O app passa `--js-runtimes` automaticamente se achar um deles no `PATH`.

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

**Privacidade do build:** o compilador costuma embutir no `.exe` caminhos da máquina que compilou (como `C:\Users\<você>\.cargo\...`). O script reescreve esses prefixos e, ao final, **aborta** se achar o usuário do Windows, a pasta do projeto ou o nome do computador dentro dos executáveis ou do instalador. Para uma distribuição pública, o ideal é gerar o instalador em um ambiente limpo (por exemplo, no CI do GitHub).

> Antivírus podem bloquear o build ou o instalador resultante (falso positivo, comum em programas Rust e em instaladores sem assinatura digital).

## Conectando sua conta

Clique no avatar (canto superior direito) e em **Conectar conta do YouTube**. Uma janela de login do Google se abre; entre normalmente, com senha, verificação em duas etapas ou passkey, e ela fecha sozinha.

Se o Google bloquear o login na janela embutida, use **Importar cookie manualmente** no mesmo menu: cole o cabeçalho `Cookie` de uma requisição para `music.youtube.com` (DevTools, aba Rede).

## Privacidade e segurança

- O app **nunca vê sua senha**. O login acontece na página real do Google, dentro do WebView2, em um processo separado (`ytmusic-login.exe`) que só existe durante o login.
- A sessão do WebView2 é descartável (modo incógnito). Nada do login fica no perfil dele.
- O app guarda **somente os cookies da sessão**, criptografados com o **DPAPI do Windows**, em `%LOCALAPPDATA%\ytmusic\auth.bin`. Só o seu usuário do Windows consegue descriptografar.
- Nada é enviado para servidores de terceiros. O app conversa apenas com o YouTube e o Google (as capas vêm dos servidores de imagem deles).
- **Desconectar** apaga o `auth.bin`.

> O cookie da sessão dá acesso à sua conta Google, não só ao YouTube Music. Nunca compartilhe o `auth.bin` nem o cookie que você cola em "Importar manualmente".

## Onde ficam os dados

Tudo em `%LOCALAPPDATA%\ytmusic\`:

| Arquivo | Conteúdo |
| --- | --- |
| `auth.bin` | cookies da sessão, protegidos pelo DPAPI |
| `volume` | último volume usado |
| `cache\` | áudios baixados, limitados pelo valor configurado no menu **Configurações** (padrão: 320 MB) |
| `cache_limit` | limite de tamanho do cache, em MB |

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
| `src/app.rs` | interface (egui): layout, listas, player bar |
| `src/ytm.rs` | cliente da API interna do YouTube Music (busca, biblioteca, mix) |
| `src/player.rs` | reprodução: yt-dlp, cache, decodificação e controle de áudio (rodio) |
| `src/auth.rs` | credenciais: cookie, `SAPISIDHASH` e proteção via DPAPI |
| `src/bin/login.rs` | janela de login em WebView2 (executável separado) |

## Stack

[egui / eframe](https://github.com/emilk/egui) · [rodio](https://github.com/RustAudio/rodio) + [symphonia](https://github.com/pdeljanov/Symphonia) · [wry](https://github.com/tauri-apps/wry) + [tao](https://github.com/tauri-apps/tao) · [ureq](https://github.com/algesten/ureq) · [yt-dlp](https://github.com/yt-dlp/yt-dlp)

## Aviso

Este projeto é independente e **não é afiliado ao YouTube nem ao Google**. Ele usa a API interna do YouTube Music (a mesma do site), que não é oficial e pode mudar sem aviso. Feito para uso pessoal; respeite os Termos de Serviço do YouTube.
