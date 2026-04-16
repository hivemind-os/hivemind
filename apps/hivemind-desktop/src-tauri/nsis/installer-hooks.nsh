; NSIS installer hooks for HiveMind OS
;
; NSIS_HOOK_POSTCOPY runs after the application files are copied to $INSTDIR.
; We use it to silently install the Microsoft Visual C++ Redistributable that
; was bundled alongside the app, then remove it from the install directory so
; it is not left behind for users.

!macro NSIS_HOOK_POSTCOPY
  ExecWait '"$INSTDIR\vc_redist.exe" /install /quiet /norestart' $0
  Delete "$INSTDIR\vc_redist.exe"
  ; 0 = success, 3010 = success with reboot required — both are acceptable.
  ; Anything else is a failure: warn the user so they can install manually.
  IntCmp $0 0 vcredist_ok
  IntCmp $0 3010 vcredist_ok
  MessageBox MB_ICONEXCLAMATION|MB_OK \
    "Warning: The Microsoft Visual C++ Redistributable could not be installed \
(exit code $0). The application may not start correctly.$\n$\nPlease install \
the Visual C++ 2015-2022 Redistributable manually from microsoft.com."
  vcredist_ok:
!macroend
