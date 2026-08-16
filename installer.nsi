; NMTs 安装脚本（NSIS 3.x）
; 由 GitHub Actions 在 Windows runner 上构建后调用 makensis 生成安装包

!define APP_NAME "NMTs"
!define APP_VERSION "1.0.0"
!define APP_PUBLISHER "GreenChennai"

Name "${APP_NAME} ${APP_VERSION}"
OutFile "NMTs-${APP_VERSION}-setup.exe"
InstallDir "$PROGRAMFILES64\${APP_NAME}"
InstallDirRegKey HKLM "Software\${APP_NAME}" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!include "MUI2.nsh"

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Section "安装"
  SetOutPath "$INSTDIR"
  File "target\release\nmts.exe"
  File "README.md"
  File "LICENSE"
  File "RELEASE_NOTES.md"

  SetOutPath "$INSTDIR\config"
  File "config\default.yaml"

  SetOutPath "$INSTDIR\vendor_db"
  File "vendor_db\huawei_vrp.yaml"
  File "vendor_db\h3c_vrp.yaml"
  File "vendor_db\cisco_ios.yaml"
  File "vendor_db\dns_providers.yaml"

  WriteRegStr HKLM "Software\${APP_NAME}" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\nmts.exe"
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\nmts.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\nmts.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\RELEASE_NOTES.md"
  Delete "$INSTDIR\config\default.yaml"
  Delete "$INSTDIR\vendor_db\*.yaml"
  RMDir "$INSTDIR\config"
  RMDir "$INSTDIR\vendor_db"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  Delete "$DESKTOP\${APP_NAME}.lnk"
  DeleteRegKey HKLM "Software\${APP_NAME}"
SectionEnd
