; Installer for Windows (Inno Setup 6), built by .github/workflows/release.yml:
;   iscc /DVersion=0.2.0 /DExe=<pfad\zu\silberkorn.exe> /O<ausgabeordner> installer\silberkorn.iss
;
; Installs for the current user only (%LOCALAPPDATA%\Programs\Silberkorn), so neither the
; installation nor the updates need administrator rights: the app's update downloads this
; installer and runs it with /SILENT (see crates/sk-core/src/update.rs). /start=0 leaves the
; app closed afterwards (update when quitting).
;
; Settings and styles live in %APPDATA%\silberkorn, catalog, models and caches in
; %LOCALAPPDATA%\silberkorn. Uninstalling asks whether to remove them too (default: keep; a silent
; uninstall keeps them). The edits themselves are in the sidecars next to the photos and stay.

#ifndef Version
  #error "Version fehlt: iscc /DVersion=x.y.z"
#endif
#ifndef Exe
  #define Exe "..\target\dist\silberkorn.exe"
#endif

[Setup]
; Never change the AppId: it links updates and uninstalling to the installed copy.
AppId={{B6D028A8-BCE3-4D89-801A-97249BFB2B1C}
AppName=Silberkorn
AppVersion={#Version}
AppVerName=Silberkorn {#Version}
AppPublisher=Martin Schneider
AppPublisherURL=https://github.com/mschneider85/silberkorn
AppUpdatesURL=https://github.com/mschneider85/silberkorn/releases
VersionInfoVersion={#Version}
PrivilegesRequired=lowest
DefaultDirName={autopf}\Silberkorn
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
UninstallDisplayIcon={app}\silberkorn.exe
UninstallDisplayName=Silberkorn
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=force
RestartApplications=no
OutputBaseFilename=silberkorn-setup-x64

[Languages]
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Exe}"; DestDir: "{app}"; DestName: "silberkorn.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Silberkorn"; Filename: "{app}\silberkorn.exe"
Name: "{autodesktop}\Silberkorn"; Filename: "{app}\silberkorn.exe"; Tasks: desktopicon

[Run]
; Interactive: a checkbox on the last page. Silent (update): starts the app unless /start=0.
Filename: "{app}\silberkorn.exe"; Description: "{cm:LaunchProgram,Silberkorn}"; Flags: nowait postinstall; Check: StartAfterInstall

[Code]
function StartAfterInstall: Boolean;
begin
  Result := ExpandConstant('{param:start|1}') <> '0';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    if MsgBox('Auch die persönlichen Daten von Silberkorn löschen?' + #13#10#13#10 +
              'Das sind Einstellungen, Stile, Exportrezepte, Katalog und Sessions-Liste, ' +
              'geladene KI-Modelle und Vorschauen. Ihre Fotos und die Bearbeitungen in den ' +
              'Sidecar-Dateien neben den Fotos bleiben erhalten.' + #13#10#13#10 +
              'Bei „Nein“ sind nach einer erneuten Installation alle Einstellungen wieder da.',
              mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    begin
      DelTree(ExpandConstant('{userappdata}\silberkorn'), True, True, True);
      DelTree(ExpandConstant('{localappdata}\silberkorn'), True, True, True);
    end;
end;
