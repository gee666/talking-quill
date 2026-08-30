!include "LogicLib.nsh"
!include "nsDialogs.nsh"
!include "FileFunc.nsh"
!include "StrFunc.nsh"
!ifdef BUILD_UNINSTALLER
${UnStrStr}
!else
${StrStr}
!endif
!define /ifndef INSTALL_REGISTRY_KEY "Software\${APP_GUID}"
!define /ifndef UNINSTALL_REGISTRY_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${UNINSTALL_APP_KEY}"

!if "$%TALKING_QUILL_PERSONAL_FRESH_INSTALL%" == "1"
  !define TALKING_QUILL_PERSONAL_INSTALLER
!endif
!if "$%TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD%" == "1"
  !define TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD
!endif
!if "$%TALKING_QUILL_INSTALL_ISOLATED_VALIDATION_BUILD%" == "1"
  !define TALKING_QUILL_INSTALL_ISOLATED_VALIDATION_BUILD
!endif
!ifdef TALKING_QUILL_INSTALL_ISOLATED_VALIDATION_BUILD
  !include "${PROJECT_DIR}\..\build\installer-install-validation.nsh"
!else
  !macro TalkingQuillInitCommitFailurePoint
  !macroend
!endif

; Fresh installs and published updates share the same two-role personal runtime
; migration. The retired authority/enrollment installer is not reachable.
Var TalkingQuillNativeProgramData
Var TalkingQuillSecureTemp

; Both generated entrypoints invoke this body before SetOutPath, logging,
; InitPluginsDir, or any plugin-backed macro. The first process uses the NSIS
; built-in ExecShellWait runas path. Only that elevated child may ask system
; PowerShell to create the protected ProgramData leaf. The final child validates
; the leaf before assigning TEMP, so privileged plugins never use user TEMP.
!macro TalkingQuillProtectedEarlyBootstrap
  ${GetParameters} $R0
  ${GetOptions} $R0 "/TQELEVATEDBOOTSTRAP=" $R1
  ${If} $R1 != "1"
    ClearErrors
    StrCpy $R2 79
    ExecShellWait "runas" "$EXEPATH" '$R0 /TQELEVATEDBOOTSTRAP=1' SW_SHOWNORMAL $R2
    IfErrors 0 +3
      SetErrorLevel 79
      Quit
    SetErrorLevel $R2
    Quit
  ${EndIf}

  ${GetOptions} $R0 "/TQPROTECTEDTEMP=" $R1
  ${If} $R1 == ""
    ClearErrors
    StrCpy $R2 79
    ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$ErrorActionPreference=$$([Management.Automation.ActionPreference]::Stop);$$pd=[Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData);$$rng=New-Object Security.Cryptography.RNGCryptoServiceProvider;$$b=New-Object byte[] 16;$$rng.GetBytes($$b);$$rng.Dispose();$$leaf=Join-Path $$pd (\".Talking Quill.Installer-\"+(-join($$b|ForEach-Object{$$_.ToString(\"x2\")})));$$acl=New-Object Security.AccessControl.DirectorySecurity;$$acl.SetOwner((New-Object Security.Principal.SecurityIdentifier(\"S-1-5-32-544\")));$$acl.SetAccessRuleProtection($$true,$$false);foreach($$sid in @(\"S-1-5-18\",\"S-1-5-32-544\")){$$acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($$sid,\"FullControl\",\"ContainerInherit,ObjectInherit\",\"None\",\"Allow\")))};[IO.Directory]::CreateDirectory($$leaf,$$acl)|Out-Null;try{$$psi=New-Object Diagnostics.ProcessStartInfo;$$psi.FileName=$$args[0];$$psi.Arguments=$$args[1]+\" /TQPROTECTEDTEMP=`\"\"+$$leaf+\"`\"\";$$psi.UseShellExecute=$$false;$$psi.EnvironmentVariables[\"TEMP\"]=$$leaf;$$psi.EnvironmentVariables[\"TMP\"]=$$leaf;$$p=[Diagnostics.Process]::Start($$psi);$$p.WaitForExit();exit $$p.ExitCode}finally{Remove-Item -LiteralPath $$leaf -Recurse -Force -ErrorAction SilentlyContinue}" "$EXEPATH" "$R0"' $R2
    IfErrors 0 +3
      SetErrorLevel 79
      Quit
    SetErrorLevel $R2
    Quit
  ${EndIf}

  ClearErrors
  StrCpy $R2 78
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$ErrorActionPreference=$$([Management.Automation.ActionPreference]::Stop);$$p=[IO.Path]::GetFullPath($$args[0]);$$pd=[Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData).TrimEnd(\"\\\");if([IO.Path]::GetDirectoryName($$p) -cne $$pd -or -not [IO.Path]::GetFileName($$p).StartsWith(\".Talking Quill.Installer-\",[StringComparison]::Ordinal)){exit 78};$$i=Get-Item -Force -LiteralPath $$p;if(($$i.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or -not $$i.PSIsContainer){exit 78};$$a=Get-Acl -LiteralPath $$p;if(-not $$a.AreAccessRulesProtected -or @($$a.Access|Where-Object{$$_.AccessControlType -ne \"Allow\" -or $$_.IdentityReference.Value -notin @(\"S-1-5-18\",\"S-1-5-32-544\") -or $$_.FileSystemRights -ne \"FullControl\"}).Count -ne 0 -or $$a.Owner -notin @(\"S-1-5-18\",\"S-1-5-32-544\")){exit 78}" "$R1"' $R2
  IfErrors 0 +2
    StrCpy $R2 78
  ${If} $R2 != 0
    SetErrorLevel 78
    Abort
  ${EndIf}
  ; $TEMP is an NSIS shell variable and cannot be a StrCpy destination. The
  ; protected child inherits TEMP/TMP before NSIS initializes $TEMP, so require
  ; that immutable value to match the validated command-line path. Otherwise
  ; InitPluginsDir could still use an unprotected directory.
  ${If} $TEMP != $R1
    SetErrorLevel 78
    Abort
  ${EndIf}
  ${GetParent} $R1 $TalkingQuillNativeProgramData
  StrCpy $TalkingQuillSecureTemp $R1
!macroend

!ifdef BUILD_UNINSTALLER
!macro customUnEarlyInit
  !insertmacro TalkingQuillProtectedEarlyBootstrap
!macroend
!else
!macro customEarlyInit
  !insertmacro TalkingQuillProtectedEarlyBootstrap
!macroend
!endif

!macro TalkingQuillRunMachineCleanup mode failureLabel
  InitPluginsDir
  !ifdef TALKING_QUILL_INSTALL_ISOLATED_VALIDATION_BUILD
    ; Transaction fault injection is source-only. Published installers never
    ; extract or execute this PowerShell fixture.
    File /oname=$PLUGINSDIR\talking-quill-machine-cleanup-test.ps1 "${PROJECT_DIR}\..\build\windows-personal-machine-cleanup.ps1"
    nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\talking-quill-machine-cleanup-test.ps1" -Mode ${mode} -FailurePoint "$PersonalCommitFailurePoint" -TestRoot "$TalkingQuillNativeProgramData"'
  !else
    ; The signed package embeds the compiled gateway as the only privileged
    ; lifecycle implementation. It derives Program Files through Known Folder,
    ; validates the extracted manifest and role hashes, and owns rollback.
    File /oname=$PLUGINSDIR\talking-quill-installer-lifecycle.exe "${PROJECT_DIR}\native\talking-quill-helper.exe"
    !ifdef BUILD_UNINSTALLER
      nsExec::ExecToStack '"$PLUGINSDIR\talking-quill-installer-lifecycle.exe" --windows-installer-lifecycle-v1=${mode}'
    !else
      nsExec::ExecToStack '"$PLUGINSDIR\talking-quill-installer-lifecycle.exe" --windows-installer-lifecycle-v1=${mode} --expected-gateway=$TalkingQuillExpectedGatewayHash --expected-owner=$TalkingQuillExpectedOwnerHash --expected-layout=$TalkingQuillExpectedLayoutDigest'
    !endif
  !endif
  Pop $0
  Pop $1
  DetailPrint "$1"
  ${If} $0 != 0
    Goto ${failureLabel}
  ${EndIf}
!macroend

!macro TalkingQuillRequestPlannedNeutralExit failureLabel
  IfFileExists "$INSTDIR\${APP_FILENAME}.exe" 0 +5
  InitPluginsDir
  ${StdUtils.ExecShellAsUser} $0 "$INSTDIR\${APP_FILENAME}.exe" "open" "--talking-quill-request-machine-quit"
  ${If} $0 != 0
    Goto ${failureLabel}
  ${EndIf}
!macroend

!macro TalkingQuillCleanCommittedRecoveryBestEffort
  !insertmacro TalkingQuillRunMachineCleanup install-retire-backup personal_backup_retirement_failed
  Goto personal_backup_retirement_done
  personal_backup_retirement_failed:
    ; The replacement is already committed and rollback is disarmed. A locked
    ; recovery tree is safe to retain for a later repair rather than failing the install.
    DetailPrint "Talking Quill committed successfully, but its recovery tree could not be retired: $1"
  personal_backup_retirement_done:
!macroend

!ifndef BUILD_UNINSTALLER
!define MUI_CUSTOMFUNCTION_ABORT TalkingQuillOnUserAbort
Var PersonalCommitFailurePoint
Var PersonalInstallTransactionArmed
Var PersonalInstallRollbackStarted
Var PersonalInstallRollbackFailed
Var TalkingQuillExpectedGatewayHash
Var TalkingQuillExpectedOwnerHash
Var TalkingQuillExpectedLayoutDigest
Var TalkingQuillMaintenanceMode

!macro customInit
  StrCpy $PersonalCommitFailurePoint "none"
  StrCpy $PersonalInstallTransactionArmed "0"
  StrCpy $PersonalInstallRollbackStarted "0"
  StrCpy $PersonalInstallRollbackFailed "0"
  StrCpy $TalkingQuillExpectedGatewayHash ""
  StrCpy $TalkingQuillExpectedOwnerHash ""
  StrCpy $TalkingQuillExpectedLayoutDigest ""
  StrCpy $TalkingQuillMaintenanceMode "install"
  ${GetParameters} $R0
  ${GetOptions} $R0 "/TQGATEWAYHASH=" $TalkingQuillExpectedGatewayHash
  ${GetOptions} $R0 "/TQOWNERHASH=" $TalkingQuillExpectedOwnerHash
  ${GetOptions} $R0 "/TQLAYOUT=" $TalkingQuillExpectedLayoutDigest
  ${GetOptions} $R0 "/TQMODE=" $R1
  ${If} $R1 != ""
    ${If} $R1 != "repair"
      SetErrorLevel 64
      Abort "Talking Quill maintenance mode is invalid."
    ${EndIf}
    StrCpy $TalkingQuillMaintenanceMode "repair"
  ${EndIf}
  !insertmacro TalkingQuillInitCommitFailurePoint
!macroend

; Patched installSection.nsh invokes this as the first application-mutating
; install-section hook, after all cancellable assisted-installer pages and before
; electron-builder's old-uninstaller discovery. This personal lifecycle owns the
; replacement, so returning from this hook intentionally bypasses discovery.
!macro customUninstallOldVersion
  ; .onInit points the NSIS process at INSTDIR. Windows will not rename a
  ; directory while the installer itself has it as its current directory.
  ; Leave the predecessor tree before the transactional cleanup tries to move it.
  SetOutPath "$TEMP"
  System::Call 'Kernel32::SetCurrentDirectoryW(w "$TEMP") i .r4'
  ${If} $4 == 0
    ${IfNot} ${Silent}
      MessageBox MB_OK|MB_ICONSTOP "Talking Quill could not leave its existing installation directory. Restart Windows, then retry." /SD IDOK
    ${EndIf}
    SetErrorLevel 70
    Abort
  ${EndIf}
  ; Personal install ignores individual-user state and replaces only fixed
  ; machine files. Existing per-user copies and registrations remain untouched.
  ; Updates first ask the medium-integrity application to perform its planned
  ; gateway and owner neutral exit. The compiled lifecycle then proves all three
  ; exact installed runtime paths are absent before mutation.
  !ifndef TALKING_QUILL_PERSONAL_INSTALLER
    !insertmacro TalkingQuillRequestPlannedNeutralExit personal_cleanup_failed
  !endif
  !ifdef TALKING_QUILL_PERSONAL_INSTALLER
    !insertmacro TalkingQuillRunMachineCleanup fresh-install personal_cleanup_failed
  !else
    ${If} $TalkingQuillMaintenanceMode == "repair"
      !insertmacro TalkingQuillRunMachineCleanup repair personal_cleanup_failed
    ${Else}
      !insertmacro TalkingQuillRunMachineCleanup install personal_cleanup_failed
    ${EndIf}
  !endif
  ; Any later InstFiles cancellation or failure must restore the moved tree.
  StrCpy $PersonalInstallTransactionArmed "1"
  ; Skip the generated HKEY_CURRENT_USER old-uninstaller branch too. Registry
  ; and file writes remain compiled for the all-users shell context.
  StrCpy $installMode "personal-machine"
  Goto personal_cleanup_done

  personal_cleanup_failed:
    ; Cleanup can fail after creating recovery state. Arm failure rollback even
    ; though the barrier did not return successfully.
    StrCpy $PersonalInstallTransactionArmed "1"
    ${IfNot} ${Silent}
      MessageBox MB_OK|MB_ICONSTOP "Talking Quill could not stop or replace its fixed machine files.$\r$\n$\r$\n$1" /SD IDOK
    ${EndIf}
    SetErrorLevel 70
    Abort
  personal_cleanup_done:
!macroend

Function TalkingQuillRollbackArmedInstall
  StrCpy $PersonalInstallRollbackFailed "0"
  ${If} $PersonalInstallTransactionArmed != "1"
    Return
  ${EndIf}
  ${If} $PersonalInstallRollbackStarted == "1"
    Return
  ${EndIf}
  StrCpy $PersonalInstallRollbackStarted "1"
  !insertmacro TalkingQuillRunMachineCleanup install-rollback personal_armed_rollback_failed
  StrCpy $PersonalInstallTransactionArmed "0"
  Return

  personal_armed_rollback_failed:
    StrCpy $PersonalInstallRollbackFailed "1"
    StrCpy $PersonalInstallRollbackStarted "0"
    SetErrorLevel 70
    MessageBox MB_OK|MB_ICONSTOP "Talking Quill could not restore the predecessor automatically. Keep this installer open and retry; its next rollback attempt uses the protected recovery tree.$\r$\n$\r$\n$1" /SD IDOK
FunctionEnd

Function TalkingQuillRollbackUntilRestored
  rollback_until_restored:
    Call TalkingQuillRollbackArmedInstall
    ${If} $PersonalInstallRollbackFailed == "1"
      Sleep 500
      Goto rollback_until_restored
    ${EndIf}
FunctionEnd

!macro customInstallFailure
  ; Generated failure branches use Quit, so they cannot regain control until
  ; the durable predecessor has been restored.
  Call TalkingQuillRollbackUntilRestored
!macroend

Function TalkingQuillOnUserAbort
  ; The install section has no cancellable page before the barrier. Once armed,
  ; a cancellation cannot exit until rollback succeeds.
  Call TalkingQuillRollbackUntilRestored
FunctionEnd

Function .onInstFailed
  ; A failed-install callback has no documented veto. Do not return from it
  ; while rollback remains armed or failed.
  Call TalkingQuillRollbackUntilRestored
FunctionEnd

!macro customInstallCommit
  ; Do not keep the installer current directory inside the replacement tree.
  ; Rollback must be able to remove a partially copied INSTDIR.
  SetOutPath "$TEMP"
  ; Commit after application files are copied, but before registry, shortcuts,
  ; associations, or launch can publish the candidate.
  !insertmacro TalkingQuillRunMachineCleanup install-commit personal_commit_failed
  ; install-commit snapshots legacy state, disables and stops its launch paths,
  ; quarantines its files, records the durable commit, then deletes the retired
  ; registrations. It does not return while mixed authority could launch.
  StrCpy $PersonalInstallTransactionArmed "0"
  !insertmacro TalkingQuillCleanCommittedRecoveryBestEffort
  StrCpy $installMode "all"
  Goto personal_commit_done

  personal_commit_failed:
    ; Abort invokes .onInstFailed, which owns the guarded rollback retry loop.
    SetErrorLevel 70
    Abort
  personal_commit_done:
!macroend

!macro customInstall
  ${IfNot} ${Silent}
    ${StdUtils.ExecShellAsUser} $0 "$launchLink" "open" ""
  ${EndIf}
!macroend
!endif

!ifdef BUILD_UNINSTALLER
Var DeleteTalkingQuillData
Var DeleteTalkingQuillDataCheckbox
Var DeleteTalkingQuillDataConfirmedByCommand
!ifdef TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD
Var TalkingQuillTestEvidenceRoot
!endif

!macro customUnInit
  StrCpy $DeleteTalkingQuillData "0"
  StrCpy $DeleteTalkingQuillDataConfirmedByCommand "0"
  ${GetParameters} $R0
  ${UnStrStr} $0 $R0 "--delete-app-data"
  ${If} $0 != ""
    SetErrorLevel 64
    Abort "Personal-data removal is available only through the uninstall checkbox and confirmation."
  ${EndIf}
  !ifdef TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD
    StrCpy $TalkingQuillTestEvidenceRoot ""
    ${GetOptions} $R0 "/TALKINGQUILLTESTROOT=" $TalkingQuillTestEvidenceRoot
    ReadEnvStr $1 "TALKING_QUILL_UNINSTALL_VALIDATION_ROOT"
    ${If} $TalkingQuillTestEvidenceRoot == ""
      StrCpy $TalkingQuillTestEvidenceRoot $1
    ${EndIf}
    ${UnStrStr} $0 $R0 "/DELETEAPPDATA=1"
    ReadEnvStr $1 "TALKING_QUILL_UNINSTALL_VALIDATION_DELETE"
    ${If} $TalkingQuillTestEvidenceRoot != ""
    ${AndIf} $0 != ""
    ${OrIf} $TalkingQuillTestEvidenceRoot != ""
    ${AndIf} $1 == "1"
      StrCpy $DeleteTalkingQuillData ${BST_CHECKED}
      StrCpy $DeleteTalkingQuillDataConfirmedByCommand "1"
    ${EndIf}
  !else
    ${UnStrStr} $0 $R0 "/DELETEAPPDATA="
    ${If} $0 != ""
      SetErrorLevel 64
      Abort "Personal-data removal is available only through the uninstall checkbox and confirmation."
    ${EndIf}
    ${UnStrStr} $0 $R0 "/TALKINGQUILLTESTROOT="
    ${If} $0 != ""
      SetErrorLevel 64
      Abort "This installer does not contain isolated uninstall test support."
    ${EndIf}
  !endif
!macroend

Function un.TalkingQuillDataPage
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    SetErrorLevel 70
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 28u "Personal data is preserved unless you explicitly choose to remove it."
  Pop $0
  ${NSD_CreateCheckbox} 0 34u 100% 42u "Also remove my Talking Quill settings, models, voice commands, recordings, dictionaries, and other personal data"
  Pop $DeleteTalkingQuillDataCheckbox
  ${NSD_Uncheck} $DeleteTalkingQuillDataCheckbox
  nsDialogs::Show
FunctionEnd

Function un.TalkingQuillDataPageLeave
  ${If} $DeleteTalkingQuillDataConfirmedByCommand != "1"
    ${NSD_GetState} $DeleteTalkingQuillDataCheckbox $DeleteTalkingQuillData
  ${EndIf}
  ${If} $DeleteTalkingQuillData == ${BST_CHECKED}
  ${AndIf} $DeleteTalkingQuillDataConfirmedByCommand != "1"
    MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 "Remove all Talking Quill personal data for the signed-in user? This cannot be undone." /SD IDNO IDYES data_removal_confirmed
    Abort
    data_removal_confirmed:
  ${EndIf}
FunctionEnd

!macro customUnWelcomePage
  !insertmacro MUI_UNPAGE_WELCOME
  UninstPage custom un.TalkingQuillDataPage un.TalkingQuillDataPageLeave
!macroend

!macro customUnInstall
  ${IfNot} ${isUpdated}
    ; Run confirmed user-data reset before irreversible machine cleanup.
    ${If} $DeleteTalkingQuillData == ${BST_CHECKED}
      IfFileExists "$INSTDIR\${APP_FILENAME}.exe" 0 uninstall_reset_missing
      InitPluginsDir
      !ifdef TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD
        ${If} $TalkingQuillTestEvidenceRoot != ""
          ; Source-only validation retains its hostile-path fixture. Published
          ; uninstallers derive the current user's fixed roaming path directly.
          InitPluginsDir
          StrCpy $6 "$PLUGINSDIR\talking-quill-user-data-target.ini"
          Delete "$6"
          File /oname=$PLUGINSDIR\talking-quill-user-data-test-target.ps1 "${PROJECT_DIR}\..\build\windows-personal-user-data-test-target.ps1"
          nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\talking-quill-user-data-test-target.ps1" -Root "$TalkingQuillTestEvidenceRoot" -OutputFile "$6"'
          Pop $0
          Pop $1
          ${If} $0 != 0
            Goto uninstall_reset_target_failed
          ${EndIf}
          ReadINIStr $5 "$6" "Target" "Path"
        ${Else}
          StrCpy $5 "$APPDATA\Talking Quill"
        ${EndIf}
      !else
        StrCpy $5 "$APPDATA\Talking Quill"
      !endif
      ${If} $5 == ""
        Goto uninstall_reset_target_failed
      ${EndIf}
      !ifdef TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD
        ${If} $TalkingQuillTestEvidenceRoot != ""
          System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_ISOLATED_TEST", w "1") i .r4'
          ${If} $4 == 0
            Goto uninstall_reset_failed
          ${EndIf}
        ${EndIf}
      !endif
      StrCpy $2 "$PLUGINSDIR\talking-quill-reset.challenge"
      FileOpen $3 "$2" w
      IfErrors uninstall_reset_failed
      ; Keep challenge bytes path-independent. FileWrite's encoding must not
      ; corrupt a Unicode elevated TEMP path before Electron reads the token.
      FileWrite $3 "talking-quill-uninstall-reset-v1"
      FileClose $3
      System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_RESET_CHALLENGE", w "talking-quill-uninstall-reset-v1") i .r4'
      ${If} $4 == 0
        Goto uninstall_reset_failed
      ${EndIf}
      System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_RESET_TARGET", w "$5") i .r4'
      ${If} $4 == 0
        Goto uninstall_reset_failed
      ${EndIf}
      ExecWait '"$INSTDIR\${APP_FILENAME}.exe" --talking-quill-reset-owned-data-and-exit="$2"' $0
      System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_RESET_TARGET", w "") i .r4'
      System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_RESET_CHALLENGE", w "") i .r4'
      !ifdef TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD
        System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_ISOLATED_TEST", w "") i .r4'
      !endif
      Delete "$2"
      ${If} $0 != 0
        MessageBox MB_OK|MB_ICONSTOP "Talking Quill could not remove the signed-in user's personal data. Uninstall stopped without reporting that data as deleted." /SD IDOK
        SetErrorLevel 70
        Abort
      ${EndIf}
      Goto uninstall_reset_done

      uninstall_reset_target_failed:
        MessageBox MB_OK|MB_ICONSTOP "Talking Quill could not validate the signed-in user's exact personal-data folder. No personal data or machine files were removed." /SD IDOK
        SetErrorLevel 70
        Abort
      uninstall_reset_missing:
        MessageBox MB_OK|MB_ICONSTOP "Talking Quill cannot remove personal data because the installed application is missing. Reinstall, then uninstall again with data removal selected." /SD IDOK
        SetErrorLevel 70
        Abort
      uninstall_reset_failed:
        System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_RESET_TARGET", w "") i .r4'
        System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_RESET_CHALLENGE", w "") i .r4'
        !ifdef TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD
          System::Call 'Kernel32::SetEnvironmentVariableW(w "TALKING_QUILL_UNINSTALL_ISOLATED_TEST", w "") i .r4'
        !endif
        MessageBox MB_OK|MB_ICONSTOP "Talking Quill could not prepare confirmed personal-data removal. Uninstall stopped." /SD IDOK
        SetErrorLevel 70
        Abort
      uninstall_reset_done:
    ${EndIf}

    !insertmacro TalkingQuillRequestPlannedNeutralExit uninstall_machine_cleanup_failed
    !insertmacro TalkingQuillRunMachineCleanup uninstall uninstall_machine_cleanup_failed
    Goto uninstall_custom_done

    uninstall_machine_cleanup_failed:
      MessageBox MB_OK|MB_ICONSTOP "Talking Quill could not stop or remove its installed runtime files.$\r$\n$\r$\n$1" /SD IDOK
      SetErrorLevel 70
      Abort
    uninstall_custom_done:
  ${EndIf}
!macroend

!macro customRemoveFiles
  SetOutPath "$TEMP"
  StrCpy $R8 0
  uninstall_program_files_retry:
    RMDir /r "$INSTDIR"
    IfFileExists "$INSTDIR" 0 uninstall_program_files_done
    IntOp $R8 $R8 + 1
    Sleep 250
    IntCmp $R8 40 uninstall_program_files_failed uninstall_program_files_retry uninstall_program_files_failed
  uninstall_program_files_failed:
    MessageBox MB_OK|MB_ICONSTOP "Windows still has a Talking Quill machine file open. Close Talking Quill, talking-quill-helper, and talking-quill-keyboard-owner, or restart Windows, then retry." /SD IDOK
    SetErrorLevel 75
    Abort
  uninstall_program_files_done:
  DeleteRegKey HKLM "${UNINSTALL_REGISTRY_KEY}"
  DeleteRegKey HKLM "${INSTALL_REGISTRY_KEY}"
!macroend
!endif
