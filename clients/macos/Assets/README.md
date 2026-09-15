# App icon

The source logo is `crates/lns-service/assets/lnsTemplate@2x.png`.
Packaging copies its unchanged bytes as `lnsTemplate.png`; the app loads it
explicitly and sets its logical size to 16 points and its template flag.

`scripts/render-icons.swift` draws that logo on a light rounded square at all
standard and Retina icon sizes. Packaging uses Apple's `iconutil` to compile
those images into `LNS.icns`. The app explicitly assigns that image at startup,
including when its executable is launched directly from a terminal.

`make icon-smoke` checks the generated icon representations and uses AppKit to
decode and render both icons from a relocated app bundle.
