; Muxr setup wizard (Inno Setup 6), published with every release as
; muxr-<version>-windows-x64-setup.exe.
;
;   iscc /DAppVersion=0.0.1 /DBinDir=..\..\target\release /DOutDir=..\..\dist packaging\windows\muxr.iss
;
; The self-update (src/update.rs) runs a newer copy of this setup silently over an install
; made with it, then this setup starts Muxr again (see [Run]).

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
AppName=Muxr
AppVersion={#AppVersion}
AppVerName=Muxr {#AppVersion}
AppPublisher=Muxr
AppPublisherURL=https://github.com/sudo-py-dev/muxr
AppSupportURL=https://github.com/sudo-py-dev/muxr/issues
AppUpdatesURL=https://github.com/sudo-py-dev/muxr/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\Muxr
DefaultGroupName=Muxr
DisableProgramGroupPage=yes
; Muxr needs no administrator rights, so it installs for the current user by default
; (and updates without a UAC prompt); the wizard still offers an install for all users.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=..\..\LICENSE-MIT
SetupIconFile=..\icons\muxr.ico
UninstallDisplayIcon={app}\muxr.ico
UninstallDisplayName=Muxr
WizardStyle=modern
; A running Muxr is closed for the upgrade and started again afterwards.
CloseApplications=yes
RestartApplications=yes
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutDir}
OutputBaseFilename=muxr-{#AppVersion}-windows-x64-setup
ShowLanguageDialog=auto

[Languages]
; The languages Muxr itself has; the wizard follows the Windows language.
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BinDir}\muxr.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\icons\muxr.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; DestName: "LICENSE-MIT.txt"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; DestName: "LICENSE-APACHE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Muxr"; Filename: "{app}\muxr.exe"; IconFilename: "{app}\muxr.ico"; Comment: "Video, audio, image, document and disc converter"
Name: "{autodesktop}\Muxr"; Filename: "{app}\muxr.exe"; IconFilename: "{app}\muxr.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\muxr.exe"; Description: "{cm:LaunchProgram,Muxr}"; Flags: nowait postinstall skipifsilent
; An update Muxr started (silent): Muxr closed for it, so start it again.
Filename: "{app}\muxr.exe"; Flags: nowait runasoriginaluser; Check: WizardSilent

[UninstallDelete]
; Updates Muxr downloaded but did not install yet (dirs::data_dir is %APPDATA%).
Type: filesandordirs; Name: "{userappdata}\muxr\updates"
