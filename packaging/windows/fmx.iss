; FMX setup wizard (Inno Setup 6), published with every release as
; fmx-<version>-windows-x64-setup.exe.
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
Compression=lzma2/max
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

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BinDir}\fmx.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\icons\fmx.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; DestName: "LICENSE-MIT.txt"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; DestName: "LICENSE-APACHE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\FMX"; Filename: "{app}\fmx.exe"; IconFilename: "{app}\fmx.ico"; Comment: "Free Media Xpress - Video, audio, image, document and disc converter"
Name: "{autodesktop}\FMX"; Filename: "{app}\fmx.exe"; IconFilename: "{app}\fmx.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\fmx.exe"; Description: "{cm:LaunchProgram,FMX}"; Flags: nowait postinstall skipifsilent
; An update FMX started (silent): FMX closed for it, so start it again.
Filename: "{app}\fmx.exe"; Flags: nowait runasoriginaluser; Check: WizardSilent

[UninstallDelete]
; Updates FMX downloaded but did not install yet (dirs::data_dir is %APPDATA%).
Type: filesandordirs; Name: "{userappdata}\fmx\updates"
