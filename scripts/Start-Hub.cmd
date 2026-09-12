@echo off
setlocal
chcp 65001 >nul
if not exist "%~dp0moyai-hub.exe" (
  echo moyai-hub.exe was not found beside this launcher.
  echo Keep Start-Hub.cmd and moyai-hub.exe in the same folder.
  pause
  exit /b 1
)
"%~dp0moyai-hub.exe" --launch %*
if errorlevel 1 (
  echo.
  echo Hub could not finish starting. See the message above.
  pause
  exit /b 1
)
exit /b 0
