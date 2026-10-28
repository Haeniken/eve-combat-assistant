; Native per-user installer. Only this installation's files/processes are removed.
Unicode true
RequestExecutionLevel user
ManifestDPIAware true
SetCompressor /SOLID lzma

!include MUI2.nsh
!include LogicLib.nsh
!include FileFunc.nsh
!include nsDialogs.nsh
!include x64.nsh

!ifndef PRODUCT_ID
  !define PRODUCT_ID "EVECombatAssistant"
!endif
!ifndef DATA_FOLDER
  !define DATA_FOLDER "EVECombatAssistant"
!endif
!ifndef PAYLOAD
  !define PAYLOAD "../dist/EVECombatAssistant"
!endif
!ifndef OUTPUT
  !define OUTPUT "../dist/EVECombatAssistant-Setup-x64.exe"
!endif

!ifndef PRODUCT_NAME
  !define PRODUCT_NAME "EVE Combat Assistant"
!endif
!define PRODUCT_VERSION "0.3.1"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}"
!define BACKGROUND "09131D"
!define FOREGROUND "D7E5ED"
!define ACCENT "89DCEF"

Name "${PRODUCT_NAME}"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\Programs\${PRODUCT_ID}"
InstallDirRegKey HKCU "Software\${PRODUCT_ID}" "InstallDir"
BrandingText " "
ShowInstDetails hide
ShowUninstDetails hide
InstallColors ${FOREGROUND} ${BACKGROUND}
VIProductVersion "0.3.1.0"
VIAddVersionKey ProductName "${PRODUCT_NAME}"
VIAddVersionKey FileDescription "Tactical overlay installer"
VIAddVersionKey FileVersion "${PRODUCT_VERSION}"
VIAddVersionKey LegalCopyright "Copyright (c) Haeniken"

Var RemoveData
Var RemoveDataCheckbox
Var Dialog
Var StatusFont

!define MUI_BGCOLOR "${BACKGROUND}"
!define MUI_TEXTCOLOR "${FOREGROUND}"
!define MUI_INSTFILESPAGE_COLORS "${FOREGROUND} ${BACKGROUND}"
!define MUI_FORCECLASSICCONTROLS
!define MUI_DIRECTORYPAGE_BGCOLOR "102331"
!define MUI_DIRECTORYPAGE_TEXTCOLOR "${FOREGROUND}"
!define MUI_ICON "assets/icon.ico"
!define MUI_UNICON "assets/icon.ico"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_RIGHT
!define MUI_HEADERIMAGE_BITMAP "assets/header.bmp"
!define MUI_WELCOMEFINISHPAGE_BITMAP "assets/sidebar.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "assets/sidebar.bmp"
!define MUI_INSTFILESPAGE_PROGRESSBAR "colored"
!define MUI_ABORTWARNING
!define MUI_CUSTOMFUNCTION_GUIINIT ThemeInit
!define MUI_CUSTOMFUNCTION_UNGUIINIT un.ThemeInit

!define MUI_WELCOMEPAGE_TITLE "Тактические системы для соло-PvP"
!define MUI_WELCOMEPAGE_TEXT "Дистанция, боеприпасы и варианты фитов — рядом с боевым интерфейсом.$\r$\n$\r$\nУстановка займёт меньше минуты. Приложение устанавливается для текущего пользователя."
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_SHOW ThemeDirectory
!insertmacro MUI_PAGE_DIRECTORY
!define MUI_PAGE_HEADER_TEXT "Подготовка к вылету"
!define MUI_PAGE_HEADER_SUBTEXT "Развёртывание бортовых тактических систем"
!define MUI_PAGE_CUSTOMFUNCTION_SHOW InstallProgress
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TITLE "Системы готовы к вылету"
!define MUI_FINISHPAGE_TEXT "Открой шестерёнку, добавь свой фит и заранее загрузи статистику нужных корпусов."
!define MUI_FINISHPAGE_RUN "$INSTDIR\eve-combat-assistant.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Запустить EVE Combat Assistant"
!define MUI_FINISHPAGE_RUN_NOTCHECKED
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
UninstPage custom un.DataPage un.DataPageLeave
!define MUI_PAGE_CUSTOMFUNCTION_SHOW un.InstallProgress
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "Russian"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "Нужна 64-разрядная Windows."
    Abort
  ${EndIf}
  SetRegView 64
  SetShellVarContext current
FunctionEnd

; The OS theme paints green over custom progress colors. Disable it just for the bar.
!macro ThemeFunctions PREFIX
Function ${PREFIX}ThemeInit
  CreateFont $StatusFont "Consolas" 10 400
  SetCtlColors $HWNDPARENT ${FOREGROUND} ${BACKGROUND}
  System::Call 'dwmapi::DwmSetWindowAttribute(p $HWNDPARENT, i 20, *i 1, i 4)'
  GetDlgItem $0 $HWNDPARENT 1028
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 1037
  SetCtlColors $0 ${ACCENT} ${BACKGROUND}
  GetDlgItem $0 $HWNDPARENT 1256
  SetCtlColors $0 "7794A6" ${BACKGROUND}
  CreateFont $1 "Consolas" 7 400
  SendMessage $0 ${WM_SETFONT} $1 0
FunctionEnd

Function ${PREFIX}InstallProgress
  FindWindow $0 "#32770" "" $HWNDPARENT
  SetCtlColors $0 ${FOREGROUND} ${BACKGROUND}
  GetDlgItem $1 $0 1004
  System::Call 'uxtheme::SetWindowTheme(p $1, w "", w "")'
  System::Call 'user32::GetWindowLongW(p $1, i -16) i.r2'
  IntOp $2 $2 & 0xFFFFFFFE ; Remove PBS_SMOOTH: use separate capacitor-like segments.
  System::Call 'user32::SetWindowLongW(p $1, i -16, i $2)'
  SendMessage $1 0x0409 0 0xEFDC89 ; PBM_SETBARCOLOR, Win32 COLORREF (BGR)
  SendMessage $1 0x2001 0 0x45341B ; PBM_SETBKCOLOR: visible blue-grey track
  ; Fixed gaps give the bar the same segmented look as EVE's capacitor display.
  ; The original progress control still receives all real installation updates.
  System::Alloc 16
  Pop $3
  System::Call 'user32::GetClientRect(p $1, p $3)'
  System::Call '*$3(i, i, i.r6, i.r7)'
  System::Free $3
  StrCpy $3 1
  ${DoWhile} $3 < 24
    IntOp $4 $6 * $3
    IntOp $4 $4 / 24
    ; WS_CHILD | WS_VISIBLE | SS_BLACKRECT; two-pixel dividers over the track.
    System::Call 'user32::CreateWindowExW(i 0, w "STATIC", w "", i 0x50000004, i $4, i 0, i 2, i $7, p $1, p 0, p 0, p 0) p.r5'
    IntOp $3 $3 + 1
  ${Loop}
  GetDlgItem $1 $0 1006
  SetCtlColors $1 ${ACCENT} ${BACKGROUND}
  SendMessage $1 ${WM_SETFONT} $StatusFont 1
FunctionEnd
!macroend
!insertmacro ThemeFunctions ""
!insertmacro ThemeFunctions "un."

Function ThemeDirectory
  SetCtlColors $mui.DirectoryPage ${FOREGROUND} ${BACKGROUND}
  SetCtlColors $mui.DirectoryPage.Text ${FOREGROUND} ${BACKGROUND}
  System::Call 'uxtheme::SetWindowTheme(p $mui.DirectoryPage.DirectoryBox, w " ", w " ")'
  SetCtlColors $mui.DirectoryPage.DirectoryBox ${ACCENT} ${BACKGROUND}
  SetCtlColors $mui.DirectoryPage.SpaceRequired "7794A6" ${BACKGROUND}
  SetCtlColors $mui.DirectoryPage.SpaceAvailable "7794A6" ${BACKGROUND}
FunctionEnd

Section "Бортовые системы"
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; Updates close the existing executable before replacing it.
  ${If} ${FileExists} "$INSTDIR\close_app.ps1"
    nsExec::ExecToStack '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\close_app.ps1" -InstallDirectory "$INSTDIR"'
    Pop $0
    Pop $1
    ${If} $0 != 0
      MessageBox MB_OK|MB_ICONSTOP "Не удалось закрыть приложение. Закрой его и повтори установку." /SD IDOK
      Abort
    ${EndIf}
  ${EndIf}
  DetailPrint "Подготовка дока..."
  File "../scripts/close_app.ps1"
  File "assets/icon.ico"
  DetailPrint "Загрузка тактического ядра..."
  SetDetailsPrint none
  File "${PAYLOAD}/eve-combat-assistant.exe"
  SetDetailsPrint both
  DetailPrint "Загрузка корабельных протоколов..."
  SetDetailsPrint none
  File "${PAYLOAD}/README.txt"
  File "${PAYLOAD}/THIRD_PARTY_NOTICES.md"
  File /r "${PAYLOAD}/licenses"
  SetDetailsPrint both
  DetailPrint "Настройка навигационных меток..."
  SetDetailsPrint none
  CreateDirectory "$SMPROGRAMS\${PRODUCT_ID}"
  CreateShortcut "$SMPROGRAMS\${PRODUCT_ID}\EVE Combat Assistant.lnk" "$INSTDIR\eve-combat-assistant.exe" "" "$INSTDIR\icon.ico"
  CreateShortcut "$SMPROGRAMS\${PRODUCT_ID}\Демонстрация.lnk" "$INSTDIR\eve-combat-assistant.exe" "--demo" "$INSTDIR\icon.ico"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\${PRODUCT_ID}\Удалить.lnk" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\${PRODUCT_ID}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Haeniken"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '$\"$INSTDIR\Uninstall.exe$\" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $0
  SetDetailsPrint both
  DetailPrint "Системы готовы к вылету."
SectionEnd

Function un.onInit
  SetRegView 64
  SetShellVarContext current
  StrCpy $RemoveData ${BST_UNCHECKED}
  ; Silent removal preserves data unless explicitly passed /REMOVE_DATA=1.
  ${GetParameters} $0
  ${GetOptions} $0 "/REMOVE_DATA=" $1
  ${If} $1 == "1"
    StrCpy $RemoveData ${BST_CHECKED}
  ${EndIf}
FunctionEnd

Function un.DataPage
  !insertmacro MUI_HEADER_TEXT "Локальные данные" "Выбери, что удалить вместе с программой"
  nsDialogs::Create 1018
  Pop $Dialog
  SetCtlColors $Dialog ${FOREGROUND} ${BACKGROUND}
  ${NSD_CreateLabel} 0 8u 100% 40u "Программа и её фоновые потоки будут остановлены.$\r$\nНастройки и база сохраняются для повторной установки."
  Pop $0
  SetCtlColors $0 ${FOREGROUND} ${BACKGROUND}
  ${NSD_CreateCheckbox} 0 60u 100% 24u "Удалить настройки, кеш и локальную базу данных"
  Pop $RemoveDataCheckbox
  System::Call 'uxtheme::SetWindowTheme(p $RemoveDataCheckbox, w "", w "")'
  SetCtlColors $RemoveDataCheckbox ${ACCENT} ${BACKGROUND}
  ${NSD_SetState} $RemoveDataCheckbox $RemoveData
  ${NSD_CreateLabel} 0 96u 100% 30u "$LOCALAPPDATA\${DATA_FOLDER}"
  Pop $0
  SetCtlColors $0 "7794A6" ${BACKGROUND}
  nsDialogs::Show
FunctionEnd

Function un.DataPageLeave
  ${NSD_GetState} $RemoveDataCheckbox $RemoveData
FunctionEnd

Section "Uninstall"
  DetailPrint "Остановка бортовых систем..."
  nsExec::ExecToStack '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\close_app.ps1" -InstallDirectory "$INSTDIR"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось закрыть приложение. Закрой его и повтори удаление." /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
  ; Clear data before removing the uninstaller, so an I/O failure can be retried.
  ${If} $RemoveData == ${BST_CHECKED}
    DetailPrint "Очистка локальной базы и настроек..."
    RMDir /r "$LOCALAPPDATA\${DATA_FOLDER}"
    ${If} ${FileExists} "$LOCALAPPDATA\${DATA_FOLDER}\*.*"
      MessageBox MB_OK|MB_ICONSTOP "Не удалось удалить локальные данные. Закрой другие копии программы и повтори удаление." /SD IDOK
      SetErrorLevel 1
      Abort
    ${EndIf}
  ${EndIf}
  DetailPrint "Удаление тактического пакета..."
  SetDetailsPrint none
  Delete "$INSTDIR\eve-combat-assistant.exe"
  Delete "$INSTDIR\close_app.ps1"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.md"
  RMDir /r "$INSTDIR\licenses"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${PRODUCT_ID}\EVE Combat Assistant.lnk"
  Delete "$SMPROGRAMS\${PRODUCT_ID}\Демонстрация.lnk"
  Delete "$SMPROGRAMS\${PRODUCT_ID}\Удалить.lnk"
  RMDir "$SMPROGRAMS\${PRODUCT_ID}"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "Software\${PRODUCT_ID}"
  SetDetailsPrint both
  DetailPrint "Удаление завершено."
SectionEnd
