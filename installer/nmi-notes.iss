; SPDX-License-Identifier: AGPL-3.0-or-later
;
; Inno Setup 6 script for nmi-notes. Build with build-installer.cmd (or
; `ISCC installer\nmi-notes.iss` after `cargo build --release`).
;
; Per-user install (no UAC prompt): the app keeps its data and config per
; user and registers autostart in HKCU, so a per-machine install buys nothing.

#define AppExe "nmi-notes.exe"
#ifndef SourceExe
  #define SourceExe "..\target\release\" + AppExe
#endif
#if !FileExists(SourceExe)
  #error Build the app first: cargo build --release
#endif
; Version comes from the exe's VERSIONINFO, which build.rs fills from Cargo.toml.
#define AppVersion GetStringFileInfo(SourceExe, "ProductVersion")

[Setup]
; AppId identifies the install for upgrades/uninstall: never change it.
AppId={{A79AA8E2-AE43-40DD-BA6C-61235D13328E}
AppName=nmi-notes
AppVersion={#AppVersion}
AppVerName=nmi-notes {#AppVersion}
AppPublisher=Tillman O Dickson, Jr.
AppPublisherURL=https://github.com/tillmanjr/nmi-notes
AppCopyright=Copyright (C) 2026 Tillman O Dickson, Jr.
LicenseFile=..\LICENSE
PrivilegesRequired=lowest
DefaultDirName={autopf}\nmi-notes
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#AppExe}
OutputDir=..\target\installer
OutputBaseFilename=nmi-notes-{#AppVersion}-setup
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; Closing the running app is handled in [Code] below.
CloseApplications=no

[Tasks]
Name: "startup"; Description: "Start nmi-notes when I sign in to Windows"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"
Source: "..\README.md"; DestDir: "{app}"
Source: "..\assets\fonts\OFL.txt"; DestDir: "{app}"; DestName: "FiraMono-OFL.txt"

[Icons]
Name: "{autoprograms}\nmi-notes"; Filename: "{app}\{#AppExe}"

[Run]
; The app's config.toml is the source of truth for start-with-Windows (the
; app re-applies it at each launch), so record the choice there rather than
; writing the Run key directly.
Filename: "{app}\{#AppExe}"; Parameters: "--autostart on"; Tasks: startup; Flags: runhidden waituntilterminated
Filename: "{app}\{#AppExe}"; Parameters: "--autostart off"; Tasks: not startup; Flags: runhidden waituntilterminated
Filename: "{app}\{#AppExe}"; Description: "Launch nmi-notes"; Flags: nowait postinstall skipifsilent

[Registry]
; Created by the app, not the installer: ValueType none removes it on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "nmi-notes"; ValueType: none; Flags: uninsdeletevalue

[Code]
const
  WM_CLOSE = $0010;

{ Asks a running nmi-notes to exit (its main window treats WM_CLOSE as a
  clean shutdown) and waits up to ~10 s so the exe can be replaced/removed. }
procedure CloseRunningApp();
var
  Wnd: HWND;
  Tries: Integer;
begin
  Wnd := FindWindowByClassName('NmiNotesMain');
  if Wnd = 0 then
    Exit;
  PostMessage(Wnd, WM_CLOSE, 0, 0);
  Tries := 0;
  while (FindWindowByClassName('NmiNotesMain') <> 0) and (Tries < 100) do
  begin
    Sleep(100);
    Tries := Tries + 1;
  end;
  Sleep(300); { let the process finish exiting after its window is gone }
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  CloseRunningApp();
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  CloseRunningApp();
  Result := True;
end;
