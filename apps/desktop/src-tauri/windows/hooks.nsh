!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
  CreateShortCut "$SMSTARTUP\Darbot Gen-Audio.lnk" "$INSTDIR\gen-audio.exe"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Delete "$SMSTARTUP\Darbot Gen-Audio.lnk"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "DarbotGenAudio"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
