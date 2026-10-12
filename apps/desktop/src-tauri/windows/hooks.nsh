!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Delete "$SMSTARTUP\Darbot Gen-Audio.lnk"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "DarbotGenAudio"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
