; Included only when TALKING_QUILL_INSTALL_ISOLATED_VALIDATION_BUILD=1.
; Production NSIS preprocessing never sees these test-only switches or values.
!if "$%TALKING_QUILL_FORCE_UNSUPPORTED_ARCH_VALIDATION%" == "1"
  !define TALKING_QUILL_FORCE_UNSUPPORTED_ARCH_VALIDATION
!endif
!if "$%TALKING_QUILL_FORCE_EXTRACTION_FAILURE_VALIDATION%" == "1"
  !define TALKING_QUILL_FORCE_EXTRACTION_FAILURE_VALIDATION
!endif

!macro TalkingQuillInitCommitFailurePoint
  ${GetParameters} $R0
  StrCpy $0 ""
  ${GetOptions} $R0 "/TALKINGQUILLTESTCOMMITFAIL=" $0
  ${If} $0 == ""
  ${OrIf} $0 == "none"
    StrCpy $PersonalCommitFailurePoint "none"
  ${ElseIf} $0 == "before-program-files-replace"
    StrCpy $PersonalCommitFailurePoint "before-program-files-replace"
  ${ElseIf} $0 == "after-program-files-copy"
    StrCpy $PersonalCommitFailurePoint "after-program-files-copy"
  ${Else}
    MessageBox MB_OK|MB_ICONSTOP "Invalid isolated installer failure-injection point." /SD IDOK
    SetErrorLevel 64
    Abort
  ${EndIf}
!macroend
