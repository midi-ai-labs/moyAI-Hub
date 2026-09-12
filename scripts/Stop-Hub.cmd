@echo off
setlocal
chcp 65001 >nul
if not exist "%~dp0moyai-hub.exe" (
  echo moyai-hub.exe was not found beside this launcher.
  pause
  exit /b 1
)
"%~dp0moyai-hub.exe" --stop %*
if errorlevel 1 (
  echo.
  echo Hub could not confirm shutdown. See the message above.
  pause
  exit /b 1
)
exit /b 0
