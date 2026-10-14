; Explorer context menu: "Compress with Rcomp" on every file and folder,
; "Decompress with Rcomp" on supported archives. Included by Tauri's NSIS
; template through `bundle.windows.nsis.installerHooks`.
;
; SHCTX follows the install mode (HKCU for per-user installs, HKLM for
; per-machine ones). Each selected item launches Rcomp once; the
; single-instance plugin folds those launches into the running app, which
; batches them into one archive.

!macro RCOMP_COMPRESS_VERB ROOT
  WriteRegStr SHCTX "Software\Classes\${ROOT}\shell\RcompCompress" "" "Compress with Rcomp"
  WriteRegStr SHCTX "Software\Classes\${ROOT}\shell\RcompCompress" "Icon" '"$INSTDIR\${MAINBINARYNAME}.exe",0'
  WriteRegStr SHCTX "Software\Classes\${ROOT}\shell\RcompCompress" "MultiSelectModel" "Player"
  WriteRegStr SHCTX "Software\Classes\${ROOT}\shell\RcompCompress\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" --compress "%1"'
!macroend

!macro RCOMP_DECOMPRESS_VERB EXT
  WriteRegStr SHCTX "Software\Classes\SystemFileAssociations\${EXT}\shell\RcompDecompress" "" "Decompress with Rcomp"
  WriteRegStr SHCTX "Software\Classes\SystemFileAssociations\${EXT}\shell\RcompDecompress" "Icon" '"$INSTDIR\${MAINBINARYNAME}.exe",0'
  WriteRegStr SHCTX "Software\Classes\SystemFileAssociations\${EXT}\shell\RcompDecompress" "MultiSelectModel" "Player"
  WriteRegStr SHCTX "Software\Classes\SystemFileAssociations\${EXT}\shell\RcompDecompress\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" --decompress "%1"'
!macroend

!macro RCOMP_ARCHIVE_EXTENSIONS MACRO
  !insertmacro ${MACRO} ".zip"
  !insertmacro ${MACRO} ".7z"
  !insertmacro ${MACRO} ".rar"
  !insertmacro ${MACRO} ".tar"
  !insertmacro ${MACRO} ".gz"
  !insertmacro ${MACRO} ".tgz"
  !insertmacro ${MACRO} ".bz2"
  !insertmacro ${MACRO} ".tbz2"
  !insertmacro ${MACRO} ".xz"
  !insertmacro ${MACRO} ".txz"
  !insertmacro ${MACRO} ".zst"
  !insertmacro ${MACRO} ".tzst"
  !insertmacro ${MACRO} ".lz4"
  !insertmacro ${MACRO} ".br"
!macroend

!macro RCOMP_REMOVE_DECOMPRESS_VERB EXT
  DeleteRegKey SHCTX "Software\Classes\SystemFileAssociations\${EXT}\shell\RcompDecompress"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro RCOMP_COMPRESS_VERB "*"
  !insertmacro RCOMP_COMPRESS_VERB "Directory"
  !insertmacro RCOMP_ARCHIVE_EXTENSIONS RCOMP_DECOMPRESS_VERB
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegKey SHCTX "Software\Classes\*\shell\RcompCompress"
  DeleteRegKey SHCTX "Software\Classes\Directory\shell\RcompCompress"
  !insertmacro RCOMP_ARCHIVE_EXTENSIONS RCOMP_REMOVE_DECOMPRESS_VERB
!macroend
