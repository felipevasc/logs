; Tauri 2.12 launches the verified installer before process::exit returns.
; Its default CheckIfAppIsRunning calls RmShutdown(RmForceShutdown) immediately,
; which can fail during shutdown and can terminate other applications holding
; the executable. Wait for the exact installed file instead, without killing
; processes, writing bytes, truncating it, or scheduling a reboot replacement.
; This hook is in the new installer, so older updater clients benefit too.
;
; https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.0/crates/tauri-bundler/src/bundle/windows/nsis/utils.nsh
; https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-createfilew

!ifmacrondef CheckIfAppIsRunning
  !error "Review the Tauri NSIS handoff: CheckIfAppIsRunning is no longer defined."
!endif
!macroundef CheckIfAppIsRunning

; Install/upgrade runs after SetOutPath (which creates $INSTDIR). The copied
; uninstaller does not, and its original directory may already be gone.
Var LogInsightUninstall
!macro NSIS_HOOK_PREINSTALL
  StrCpy $LogInsightUninstall 0
!macroend
!macro NSIS_HOOK_PREUNINSTALL
  StrCpy $LogInsightUninstall 1
!macroend

!macro CheckIfAppIsRunning executablePath productName
  !define LogInsightWaitID ${__COUNTER__}
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  StrCpy $2 0
  li_wait_${LogInsightWaitID}:
    ; GENERIC_WRITE, FILE_SHARE_READ|WRITE|DELETE, OPEN_EXISTING. Acquiring
    ; write access detects mapped executables and readers denying writes.
    ; OPEN_EXISTING never creates a file, and no WriteFile call is made.
    System::Call 'kernel32::CreateFileW(w "${executablePath}", i 0x40000000, i 7, p 0, i 3, i 0, p 0) p .r0 ?e'
    Pop $1
    ${If} $0 P<> -1
      System::Call 'kernel32::CloseHandle(p r0)'
      Goto li_ready_${LogInsightWaitID}
    ${EndIf}
    ${If} $1 = 2
      ; First installation: there is no previous executable.
      Goto li_ready_${LogInsightWaitID}
    ${EndIf}
    ${If} $1 = 3
    ${AndIf} $LogInsightUninstall = 1
      ; Permit PATH_NOT_FOUND only for uninstall, and only if an independent
      ; attributes query also confirms the installation directory is absent.
      ; Never turn ACCESS_DENIED (5), a sharing violation, or failed creation
      ; of an install/update directory into an unlocked-file result.
      System::Call 'kernel32::GetFileAttributesW(w "$INSTDIR") i .r3 ?e'
      Pop $4
      ${If} $3 = -1
        ${If} $4 = 2
        ${OrIf} $4 = 3
          Goto li_ready_${LogInsightWaitID}
        ${EndIf}
        StrCpy $1 $4
      ${EndIf}
    ${EndIf}
    IntOp $2 $2 + 1
    ${If} $2 < 120
      Sleep 250
      Goto li_wait_${LogInsightWaitID}
    ${EndIf}
    DetailPrint "${productName}: executable unavailable after 30 seconds (Windows error $1)."
    IfSilent li_abort_${LogInsightWaitID}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Não foi possível substituir ${productName}.$\r$\n$\r$\nFeche as outras janelas desta instalação e tente novamente. Se ela foi aberta como administrador, feche-a nessa sessão. Nenhum processo será encerrado à força.$\r$\n$\r$\nCancelar mantém a versão atual. Você também pode baixar a atualização e instalá-la depois de fechar o aplicativo.$\r$\n$\r$\nErro do Windows: $1" IDRETRY li_retry_${LogInsightWaitID} IDCANCEL li_abort_${LogInsightWaitID}
    li_retry_${LogInsightWaitID}:
      StrCpy $2 0
      Goto li_wait_${LogInsightWaitID}
    li_abort_${LogInsightWaitID}:
      Pop $4
      Pop $3
      Pop $2
      Pop $1
      Pop $0
      SetErrorLevel 2
      ; Both Cancel and a silent timeout finish with a nonzero process result,
      ; rather than leaving an aborted passive wizard waiting for another click.
      Quit
    li_ready_${LogInsightWaitID}:
      Pop $4
      Pop $3
      Pop $2
      Pop $1
      Pop $0
  !undef LogInsightWaitID
!macroend
