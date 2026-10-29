; FMX setup wizard (Inno Setup 6), published with every release as
; fmx-<version>-windows-x64-setup.exe. Saved as UTF-8 with a byte-order mark,
; which Inno Setup needs to read the translated messages below.
;
;   iscc /DAppVersion=0.0.1 /DBinDir=..\..\target\release /DOutDir=..\..\dist packaging\windows\fmx.iss
;
; The self-update (src/update.rs) runs a newer copy of this setup silently over an install
; made with it, then this setup starts FMX again (see [Run]).

#ifndef AppVersion
  #error Pass the version: /DAppVersion=x.y.z
#endif
#ifndef BinDir
  #define BinDir "..\..\target\release"
#endif
#ifndef OutDir
  #define OutDir "..\..\dist"
#endif

[Setup]
; Never change AppId: Windows tells installs apart by it, and upgrades find the old one with it.
AppId={{D051A784-A327-47BE-8A12-B2FBF998D023}
AppName=FMX
AppVersion={#AppVersion}
AppVerName=FMX {#AppVersion}
AppPublisher=Free Media Xpress
AppPublisherURL=https://github.com/sudo-py-dev/fmx
AppSupportURL=https://github.com/sudo-py-dev/fmx/issues
AppUpdatesURL=https://github.com/sudo-py-dev/fmx/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\FMX
DefaultGroupName=FMX
DisableProgramGroupPage=yes
; FMX needs no administrator rights, so it installs for the current user by default
; (and updates without a UAC prompt); the wizard still offers an install for all users.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=..\..\LICENSE-MIT
SetupIconFile=..\icons\fmx.ico
UninstallDisplayIcon={app}\fmx.ico
UninstallDisplayName=FMX
WizardStyle=modern
; A running FMX is closed for the upgrade and started again afterwards.
CloseApplications=yes
RestartApplications=yes
Compression=lzma2/ultra64
SolidCompression=yes
OutputDir={#OutDir}
OutputBaseFilename=fmx-{#AppVersion}-windows-x64-setup
ShowLanguageDialog=auto

[Languages]
; The languages FMX itself has; the wizard follows the Windows language.
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "he"; MessagesFile: "compiler:Languages\Hebrew.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "contextmenu"; Description: "{cm:ContextMenu}"

[Files]
Source: "{#BinDir}\fmx.exe"; DestDir: "{app}"; Flags: ignoreversion
; ffmpeg, ffprobe, yt-dlp and ffmpeg's DLLs, filled by packaging/fetch-bundled.py.
Source: "{#BinDir}\tools\bin\*"; DestDir: "{app}\tools\bin"; Flags: ignoreversion
Source: "..\icons\fmx.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; DestName: "LICENSE-MIT.txt"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; DestName: "LICENSE-APACHE.txt"; Flags: ignoreversion
Source: "..\THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; An upgrade replaces the bundled tools as a whole, so no outdated DLL is left behind.
Type: filesandordirs; Name: "{app}\tools"
; Unreleased 0.0.7 test builds put the tools beside fmx.exe.
Type: files; Name: "{app}\ffmpeg.exe"
Type: files; Name: "{app}\ffprobe.exe"
Type: files; Name: "{app}\yt-dlp.exe"
; Shortcuts were "FMX.lnk" up to 0.2.0. Windows keeps a file name's old case
; when it is overwritten, so remove them for the new "fmx.lnk" to take over.
Type: files; Name: "{autoprograms}\FMX.lnk"
Type: files; Name: "{autodesktop}\FMX.lnk"

[Icons]
Name: "{autoprograms}\fmx"; Filename: "{app}\fmx.exe"; IconFilename: "{app}\fmx.ico"; Comment: "Free Media Xpress - Video, audio, image, document and disc converter"
Name: "{autodesktop}\fmx"; Filename: "{app}\fmx.exe"; IconFilename: "{app}\fmx.ico"; Tasks: desktopicon

[CustomMessages]
ContextMenu=Add "Convert with FMX" to the right-click menu of files
es.ContextMenu=Añadir "Convertir con FMX" al menú contextual de los archivos
fr.ContextMenu=Ajouter « Convertir avec FMX » au menu contextuel des fichiers
de.ContextMenu=„Mit FMX konvertieren“ zum Kontextmenü von Dateien hinzufügen
he.ContextMenu=הוספת "המרה עם FMX" לתפריט הלחיצה הימנית של קבצים

[Run]
; FMX writes its right-click menu itself (src/shell.rs); unticking the task removes it.
Filename: "{app}\fmx.exe"; Parameters: "--register-shell"; Flags: runhidden waituntilterminated; Tasks: contextmenu
Filename: "{app}\fmx.exe"; Parameters: "--unregister-shell"; Flags: runhidden waituntilterminated; Tasks: not contextmenu
Filename: "{app}\fmx.exe"; Description: "{cm:LaunchProgram,FMX}"; Flags: nowait postinstall skipifsilent
; An update FMX started (silent): FMX closed for it, so start it again.
Filename: "{app}\fmx.exe"; Flags: nowait runasoriginaluser; Check: WizardSilent

[UninstallRun]
Filename: "{app}\fmx.exe"; Parameters: "--unregister-shell"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveContextMenu"

[UninstallDelete]
; Updates FMX downloaded but did not install yet (dirs::data_dir is %APPDATA%).
Type: filesandordirs; Name: "{userappdata}\fmx\updates"
