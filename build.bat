@echo off
rem Double-click to run build.sh (release build) in Git Bash.
rem From a terminal you can pass the same options: build.bat run / dev / windows / test / clean
setlocal
cd /d "%~dp0"

set "BASH="
if exist "%ProgramFiles%\Git\bin\bash.exe" set "BASH=%ProgramFiles%\Git\bin\bash.exe"
if not defined BASH if exist "%ProgramFiles(x86)%\Git\bin\bash.exe" set "BASH=%ProgramFiles(x86)%\Git\bin\bash.exe"
if not defined BASH if exist "%LocalAppData%\Programs\Git\bin\bash.exe" set "BASH=%LocalAppData%\Programs\Git\bin\bash.exe"
if not defined BASH (
  rem Fall back to the Git install on PATH (avoids WSL's System32\bash.exe).
  for /f "delims=" %%G in ('where git 2^>nul') do if not defined BASH if exist "%%~dpG..\bin\bash.exe" set "BASH=%%~dpG..\bin\bash.exe"
)
if not defined BASH (
  echo Git Bash not found. Install Git for Windows from https://git-scm.com
  pause
  exit /b 1
)

"%BASH%" ./build.sh %*
set "RC=%ERRORLEVEL%"
echo.
if not "%RC%"=="0" echo Build failed with exit code %RC%.
pause
exit /b %RC%
