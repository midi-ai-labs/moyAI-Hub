@echo off
setlocal
chcp 65001 >nul
set "hubExecutable=%~dp0moyai-hub.exe"
if not exist "%hubExecutable%" set "hubExecutable=%~dp0bin\moyai-hub.exe"
if not exist "%hubExecutable%" (
  echo Hubの実行ファイルがありません。ZIP全体を展開し、Setup-Hub.cmdを実行してください。
  pause
  exit /b 1
)
"%hubExecutable%" --launch %*
if errorlevel 1 (
  echo.
  echo Hubを起動できませんでした。上の表示と起動ログを確認してください。
  pause
  exit /b 1
)
exit /b 0
