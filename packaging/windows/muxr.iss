; Inno Setup script for the Muxr installer.
; Built by the release workflow:
;   iscc /DAppVersion=0.0.1 /DSourceExe=target\release\muxr.exe packaging\windows\muxr.iss

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z (the version from Cargo.toml)
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\muxr.exe"
#endif

[Setup]
; Never change AppId: it lets new versions upgrade the installed one.
AppId={{D051A784-A327-47BE-8A12-B2FBF998D023}
AppName=Muxr
AppVersion={#AppVersion}
AppVerName=Muxr {#AppVersion}
AppPublisher=sudo-py-dev
AppPublisherURL=https://github.com/sudo-py-dev/muxr
AppSupportURL=https://github.com/sudo-py-dev/muxr/issues
AppUpdatesURL=https://github.com/sudo-py-dev/muxr/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\Muxr
DefaultGroupName=Muxr
DisableProgramGroupPage=yes
; Install for the current user without admin rights, or for everyone.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=muxr-{#AppVersion}-windows-x64-setup
SetupIconFile=..\icons\muxr.ico
UninstallDisplayIcon={app}\muxr.exe
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; Updates run this installer while Muxr is open: close it, then restart it.
CloseApplications=yes
RestartApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "muxr.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Muxr"; Filename: "{app}\muxr.exe"
Name: "{autodesktop}\Muxr"; Filename: "{app}\muxr.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\muxr.exe"; Description: "{cm:LaunchProgram,Muxr}"; Flags: nowait postinstall skipifsilent
