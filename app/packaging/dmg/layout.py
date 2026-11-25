# dmgbuild settings for the Fathom installer. This file is only needed when
# regenerating packaging/dmg/DS_Store; normal packaging reuses the baked store.
#
#   dmgbuild -s packaging/dmg/layout.py \
#     -D app=dist/Fathom.app -D background=packaging/dmg/background.png \
#     Fathom /tmp/Fathom-layout.dmg
#
# Window: 600×360 content (+28 title bar), icons at (150,160) and (450,160).
app = defines["app"]
background_path = defines["background"]

files = [app]
symlinks = {"Applications": "/Applications"}
background = background_path
window_rect = ((440, 260), (600, 388))
icon_size = 128
icon_locations = {
    "Fathom.app": (150, 160),
    "Applications": (450, 160),
}
format = "UDZO"
