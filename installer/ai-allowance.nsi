Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"

!ifndef VERSION
!error "Pass /DVERSION=<Cargo package version>."
!endif
!ifndef OUTPUT_FILE
!define OUTPUT_FILE "..\target\installer\ai-allowance-windows-x64-setup.exe"
!endif

!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\AIAllowance"
!define START_MENU "$SMPROGRAMS\AI Allowance"

Name "AI Allowance"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\AI Allowance"
ShowInstDetails show
ShowUninstDetails show
!define MUI_ICON "..\assets\aiallowance.ico"
!define MUI_UNICON "..\assets\aiallowance.ico"
VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "AI Allowance"
VIAddVersionKey /LANG=1033 "FileDescription" "AI Allowance per-user installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "CompanyName" "Aven Solutions AI"
VIAddVersionKey /LANG=1033 "LegalCopyright" "AIAllowance contributors"

!define MUI_ABORTWARNING
!define MUI_WELCOMEFINISHPAGE_BITMAP "..\assets\installer\sidebar.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "..\assets\installer\sidebar.bmp"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_RIGHT
!define MUI_HEADERIMAGE_BITMAP "..\assets\installer\header.bmp"
!define MUI_FINISHPAGE_RUN "$INSTDIR\ai-allowance.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch AI Allowance"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; A running tray keeps its executable locked. Never terminate it or other processes.
!macro CheckNotRunning PREFIX
Function ${PREFIX}CheckNotRunning
    retry:
    System::Call 'kernel32::OpenMutexW(i 0x100000, i 0, w "Local\AIAllowance.SingleInstance") p.r0'
    ${If} $0 != 0
        System::Call 'kernel32::CloseHandle(p r0)'
        IfSilent blocked
        MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Quit AI Allowance from its tray menu before continuing." IDRETRY retry
        blocked:
        SetErrorLevel 1
        Abort
    ${EndIf}
FunctionEnd
!macroend
!insertmacro CheckNotRunning ""
!insertmacro CheckNotRunning "un."

Function .onInit
    SetShellVarContext current
    SetRegView 64
    ${IfNot} ${RunningX64}
        MessageBox MB_OK|MB_ICONSTOP "AI Allowance requires Windows x64."
        SetErrorLevel 1
        Abort
    ${EndIf}
    Call CheckNotRunning
FunctionEnd

Function un.onInit
    SetShellVarContext current
    SetRegView 64
    Call un.CheckNotRunning
FunctionEnd

Section "AI Allowance (required)" AppSection
    SectionIn RO
    Call CheckNotRunning
    SetOutPath "$INSTDIR"
    ClearErrors
    File "..\target\installer-build\release\ai-allowance.exe"
    File "..\LICENSE"
    File "..\README.md"
    WriteUninstaller "$INSTDIR\Uninstall.exe"
    CreateDirectory "${START_MENU}"
    CreateShortcut "${START_MENU}\AI Allowance.lnk" "$INSTDIR\ai-allowance.exe"
    CreateShortcut "${START_MENU}\Uninstall.lnk" "$INSTDIR\Uninstall.exe"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "AI Allowance"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Aven Solutions AI"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$\"$INSTDIR\ai-allowance.exe$\""
    WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
    WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" "$\"$INSTDIR\Uninstall.exe$\" /S"
    WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
    WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
    ${If} ${Errors}
        MessageBox MB_OK|MB_ICONSTOP "Installation failed. Files, shortcuts or the current user's uninstall registration could not be written." /SD IDOK
        SetErrorLevel 1
        Quit
    ${EndIf}
SectionEnd

Section "Start with Windows (current user only)" StartupSection
    ClearErrors
    WriteRegStr HKCU "${RUN_KEY}" "AIAllowance" "$\"$INSTDIR\ai-allowance.exe$\""
    ${If} ${Errors}
        MessageBox MB_OK|MB_ICONSTOP "Could not enable Start with Windows for the current user." /SD IDOK
        SetErrorLevel 1
        Quit
    ${EndIf}
SectionEnd

Section "Uninstall"
    Call un.CheckNotRunning
    ; Only remove our value when it still refers to this installation.
    !define /math REG_BUFFER_BYTES ${NSIS_MAX_STRLEN} * 2
    System::Call 'advapi32::RegGetValueW(p 0x80000001, w "${RUN_KEY}", w "AIAllowance", i 0x10002, p 0, w .r0, *i ${REG_BUFFER_BYTES}) i.r1'
    ${If} $1 != 0
    ${AndIf} $1 != 2
        MessageBox MB_OK|MB_ICONSTOP "Could not read the current user's startup entry (Windows error $1). Nothing has been removed." /SD IDOK
        SetErrorLevel 1
        Abort
    ${EndIf}
    ${If} $1 == 0
    ${AndIf} $0 == '$\"$INSTDIR\ai-allowance.exe$\"'
        ClearErrors
        DeleteRegValue HKCU "${RUN_KEY}" "AIAllowance"
        ${If} ${Errors}
            MessageBox MB_OK|MB_ICONSTOP "Could not remove this installation's current-user startup entry." /SD IDOK
            SetErrorLevel 1
            Abort
        ${EndIf}
    ${EndIf}
    ClearErrors
    Delete "$INSTDIR\ai-allowance.exe"
    Delete "$INSTDIR\LICENSE"
    Delete "$INSTDIR\README.md"
    Delete "${START_MENU}\AI Allowance.lnk"
    Delete "${START_MENU}\Uninstall.lnk"
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    Delete "$INSTDIR\Uninstall.exe"
    ${If} ${Errors}
        MessageBox MB_OK|MB_ICONSTOP "Some AI Allowance files or current-user registration could not be removed." /SD IDOK
        SetErrorLevel 1
        Abort
    ${EndIf}
    ; Non-recursive removal deliberately preserves config.toml and unknown files.
    RMDir "${START_MENU}"
    RMDir "$INSTDIR"
SectionEnd
