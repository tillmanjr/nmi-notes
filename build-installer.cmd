@echo off
rem SPDX-License-Identifier: AGPL-3.0-or-later
rem Release build + Windows installer -> target\installer\nmi-notes-<version>-setup.exe
rem Needs Inno Setup 6: winget install JRSoftware.InnoSetup
setlocal
cd /d "%~dp0"

set "ISCC="
for %%P in (
  "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"
  "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
  "%ProgramFiles%\Inno Setup 6\ISCC.exe"
) do if not defined ISCC if exist %%P set "ISCC=%%~P"
if not defined ISCC (
  where ISCC.exe >nul 2>&1 && set "ISCC=ISCC.exe"
)
if not defined ISCC (
  echo Inno Setup 6 not found. Install it: winget install JRSoftware.InnoSetup
  exit /b 1
)

cargo build --release || exit /b 1
"%ISCC%" /Q installer\nmi-notes.iss || exit /b 1
echo Built: target\installer
dir /b target\installer\*.exe
