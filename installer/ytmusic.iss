; Instalador do YT Music (Inno Setup 6).
; Gere com: powershell -File installer\build.ps1
; A versao vem do Cargo.toml via /DAppVersion=...

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef BinDir
  #define BinDir "..\target\release"
#endif
#define AppName "YT Music"
#define AppExe "ytmusic.exe"

[Setup]
; GUID fixo: identifica o app para atualizar/desinstalar. Nao troque.
AppId={{8E3F5A52-6C1B-4D0E-9A47-3B2F7C91D6A4}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
; Instalacao por usuario (sem admin): vai para %LOCALAPPDATA%\Programs\YT Music.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#AppExe}
SetupIconFile=..\assets\icon.ico
OutputDir=Output
OutputBaseFilename=YTMusic-Setup-{#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "installytdlp"; Description: "Instalar o yt-dlp (necessário para tocar músicas)"; GroupDescription: "Dependências (via winget):"; Check: NeedYtDlp
Name: "installdeno"; Description: "Instalar o Deno (runtime JavaScript usado pelo yt-dlp)"; GroupDescription: "Dependências (via winget):"; Check: NeedJsRuntime

[Files]
; Os dois executaveis precisam ficar na mesma pasta.
Source: "{#BinDir}\ytmusic.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\ytmusic-login.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{cmd}"; Parameters: "/C winget install -e --id yt-dlp.yt-dlp --silent --accept-source-agreements --accept-package-agreements"; StatusMsg: "Instalando o yt-dlp..."; Flags: runhidden waituntilterminated; Tasks: installytdlp
Filename: "{cmd}"; Parameters: "/C winget install -e --id DenoLand.Deno --silent --accept-source-agreements --accept-package-agreements"; StatusMsg: "Instalando o Deno..."; Flags: runhidden waituntilterminated; Tasks: installdeno
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
; Atualizacao feita pelo proprio app (instalador silencioso): reabre o app no final.
Filename: "{app}\{#AppExe}"; Flags: nowait; Check: WizardSilent

[UninstallDelete]
; Remove tambem os dados do app (cookies da sessao, volume e cache de audio).
Type: filesandordirs; Name: "{localappdata}\ytmusic"

[Code]
function HasWinget: Boolean;
begin
  Result := FileExists(ExpandConstant('{localappdata}\Microsoft\WindowsApps\winget.exe'))
         or (FileSearch('winget.exe', GetEnv('PATH')) <> '');
end;

{ Procura <prefixo>_*\<exe> na pasta de pacotes do winget. }
function WingetPackageHas(const Prefix, Exe: String): Boolean;
var
  Base: String;
  Find: TFindRec;
begin
  Result := False;
  Base := ExpandConstant('{localappdata}\Microsoft\WinGet\Packages\');
  if FindFirst(Base + Prefix + '_*', Find) then
  try
    repeat
      if FileExists(Base + Find.Name + '\' + Exe) then
      begin
        Result := True;
        Break;
      end;
    until not FindNext(Find);
  finally
    FindClose(Find);
  end;
end;

function NeedYtDlp: Boolean;
begin
  Result := HasWinget
    and not FileExists(ExpandConstant('{localappdata}\ytmusic\yt-dlp.exe'))
    and (FileSearch('yt-dlp.exe', GetEnv('PATH')) = '')
    and not WingetPackageHas('yt-dlp.yt-dlp', 'yt-dlp.exe');
end;

function NeedJsRuntime: Boolean;
begin
  Result := HasWinget
    and (FileSearch('deno.exe', GetEnv('PATH')) = '')
    and (FileSearch('node.exe', GetEnv('PATH')) = '')
    and not WingetPackageHas('DenoLand.Deno', 'deno.exe');
end;
