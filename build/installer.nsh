!include "LogicLib.nsh"
!include "nsDialogs.nsh"
!include "FileFunc.nsh"

!ifdef BUILD_UNINSTALLER

Var DeleteTalkingQuillData
Var DeleteTalkingQuillDataCheckbox

!macro customUnInit
  StrCpy $DeleteTalkingQuillData "0"
  ${GetOptions} $CMDLINE "/DELETEAPPDATA" $0
  ${IfNot} ${Errors}
    StrCpy $DeleteTalkingQuillData ${BST_CHECKED}
  ${EndIf}
!macroend

Function un.TalkingQuillDataPage
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 24u "Choose whether to keep your local Talking Quill data."
  Pop $0
  ${NSD_CreateCheckbox} 0 32u 100% 28u "Also delete my Talking Quill settings, history, credentials, models, screenshots, and logs"
  Pop $DeleteTalkingQuillDataCheckbox
  ${NSD_Uncheck} $DeleteTalkingQuillDataCheckbox
  nsDialogs::Show
FunctionEnd

Function un.TalkingQuillDataPageLeave
  ${NSD_GetState} $DeleteTalkingQuillDataCheckbox $DeleteTalkingQuillData
FunctionEnd

; electron-builder places customUninstallPage after MUI_UNPAGE_INSTFILES. Override the welcome
; slot instead so opt-in is collected before the uninstall section can remove installed files.
!macro customUnWelcomePage
  !insertmacro MUI_UNPAGE_WELCOME
  UninstPage custom un.TalkingQuillDataPage un.TalkingQuillDataPageLeave
!macroend

!macro customUnInstall
  ${If} $DeleteTalkingQuillData == ${BST_CHECKED}
    IfFileExists "$INSTDIR\${APP_FILENAME}.exe" 0 reset_helper_missing
    InitPluginsDir
    StrCpy $1 "$PLUGINSDIR\talking-quill-reset.challenge"
    FileOpen $2 "$1" w
    IfErrors reset_helper_failed
    FileWrite $2 "$PLUGINSDIR"
    FileClose $2
    System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_RESET_CHALLENGE", w "$PLUGINSDIR") i .r3'
    ${If} $3 == 0
      Goto reset_helper_failed
    ${EndIf}
    ReadEnvStr $4 "TALKING_QUILL_NSIS_EVIDENCE_ROOT"
    ExecWait '"$INSTDIR\${APP_FILENAME}.exe" --talking-quill-reset-owned-data-and-exit="$1" --talking-quill-nsis-evidence-root="$4"' $0
    System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_RESET_CHALLENGE", w "") i .r3'
    Delete "$1"
    ${If} $0 != 0
      MessageBox MB_OK|MB_ICONSTOP "Talking Quill could not verify or remove application data. Uninstall has been stopped; your data was not reported as deleted."
      Abort
    ${EndIf}
    Goto reset_helper_done

    reset_helper_missing:
      MessageBox MB_OK|MB_ICONSTOP "Talking Quill cannot remove application data because the installed reset helper is unavailable. Uninstall has been stopped."
      Abort
    reset_helper_failed:
      System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_RESET_CHALLENGE", w "") i .r3'
      MessageBox MB_OK|MB_ICONSTOP "Talking Quill could not create the one-time reset challenge. Uninstall has been stopped."
      Abort
    reset_helper_done:
  ${EndIf}
!macroend

!endif
