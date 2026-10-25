@echo off
setlocal
chcp 65001 >nul
set "hubExecutable=%~dp0moyai-hub.exe"
if not exist "%hubExecutable%" set "hubExecutable=%~dp0bin\moyai-hub.exe"
if not exist "%hubExecutable%" (
  echo Hubの実行ファイルがありません。ZIP全体を展開してください。
  pause
  exit /b 1
)
"%hubExecutable%" --stop %*
if errorlevel 1 (
  echo.
  echo Hubの終了を確認できませんでした。上の表示を確認してください。
  pause
  exit /b 1
)
exit /b 0
