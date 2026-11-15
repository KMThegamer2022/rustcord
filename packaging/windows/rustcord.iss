; Inno Setup script for the Windows installer.
; Built by GitHub Actions with:  iscc /DAppVersion=0.1.0 packaging\windows\rustcord.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

[Setup]
AppId={{6F1D2B7A-3C4E-4B8F-9A21-5C7E0D1A2B3C}}
AppName=Rustcord
AppVersion={#AppVersion}
AppPublisher=KMThegamer2022
DefaultDirName={autopf}\Rustcord
DefaultGroupName=Rustcord
OutputDir=..\..\dist
OutputBaseFilename=Rustcord-{#AppVersion}-windows-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
UninstallDisplayName=Rustcord

[Files]
Source: "..\..\target\release\rustcord.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Rustcord"; Filename: "{app}\rustcord.exe"
Name: "{autodesktop}\Rustcord"; Filename: "{app}\rustcord.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"

[Run]
Filename: "{app}\rustcord.exe"; Description: "Launch Rustcord"; Flags: nowait postinstall skipifsilent
