@echo off
setlocal
chcp 65001 >nul
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\Setup-Hub.ps1" -Interactive %*
set "setupExitCode=%errorlevel%"
pause
exit /b %setupExitCode%
