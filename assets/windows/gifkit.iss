; GIFkit installer (Inno Setup 6).
;   iscc /DAppVersion=1.8.0 /DSrc=<folder with gifkit.exe> /O<out folder> gifkit.iss
; Installs for the current user by default (no admin rights), so GIFkit can
; update itself in place. All users is offered too.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Src
  #define Src "..\..\target\release"
#endif

[Setup]
AppId={{8C2E5B71-4D3A-4F6E-9B12-7A0C3E5D9F24}
AppName=GIFkit
AppVersion={#AppVersion}
AppVerName=GIFkit {#AppVersion}
AppPublisher=zoeysmith37
AppPublisherURL=https://github.com/zoeysmith37/GIFkit
AppSupportURL=https://github.com/zoeysmith37/GIFkit/issues
AppUpdatesURL=https://github.com/zoeysmith37/GIFkit/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\GIFkit
DefaultGroupName=GIFkit
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline dialog
UsedUserAreasWarning=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputBaseFilename=gifkit-windows-x86_64-setup
SetupIconFile=..\icon\gifkit.ico
UninstallDisplayIcon={app}\gifkit.exe
UninstallDisplayName=GIFkit
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
; GIFkit itself is closed before its files are replaced; Explorer (which
; may hold the thumbnail DLL) is left alone
CloseApplications=yes
CloseApplicationsFilter=gifkit.exe
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Src}\gifkit.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Src}\gifkit_thumbs.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Src}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Src}\*.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\GIFkit"; Filename: "{app}\gifkit.exe"
Name: "{autodesktop}\GIFkit"; Filename: "{app}\gifkit.exe"; Tasks: desktopicon

[Registry]
; GIFkit writes these itself at startup; the uninstaller takes them away
Root: HKCU; Subkey: "Software\Classes\.gifkit"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\GIFkit.Project"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\CLSID\{{6F3B1C2E-9A4D-4E7B-8C51-2D7A0B9E4F13}"; Flags: uninsdeletekey

[Run]
Filename: "{app}\gifkit.exe"; Parameters: "--register"; Flags: runhidden
Filename: "{app}\gifkit.exe"; Description: "{cm:LaunchProgram,GIFkit}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; left by in-app updates
Type: files; Name: "{app}\*.old*"
