#!/bin/sh
# Haven is not signed with an Apple Developer ID, so macOS may say it is "damaged" or
# from an "unidentified developer" after a browser download. This clears the download
# flag and re-signs the app on this Mac so it opens normally.
# Run it by right-clicking this file and choosing Open (the first time only).
APP="/Applications/Haven.app"
[ -d "$APP" ] || APP="$HOME/Applications/Haven.app"
if [ ! -d "$APP" ]; then
  echo "Haven.app was not found in /Applications. Drag Haven into Applications first, then run this again."
  printf "Press Return to close."; read -r _; exit 1
fi
echo "Fixing $APP ..."
if ! xattr -cr "$APP" 2>/dev/null; then
  echo "Administrator rights are needed; macOS will ask for your password."
  sudo xattr -cr "$APP"
fi
codesign --force --deep --sign - "$APP" 2>/dev/null || sudo codesign --force --deep --sign - "$APP"
echo "Done. Opening Haven."
open "$APP"
