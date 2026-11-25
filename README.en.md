# Fathom

Fathom is a native macOS floor-plan and interior-design tool for designers, renovation teams and custom-furniture studios. Draw walls, doors and windows, place furniture, calculate areas and quotes in 2D, then inspect the same plan in 3D, under different sunlight settings, or walk through it in first person.

Local-first: no account, no cloud dependency. Plans and preferences stay on your Mac.

**0.0.2** · macOS 11+ · Apple Silicon / Intel

[Download](https://github.com/gexiaosen/Fathom/releases) · [Changelog](CHANGELOG.md)

## What's new in 0.0.2

- **Added:** Check for Updates in the More menu, a silent daily check, and a red-dot badge on the More button.
- **Added:** Automatic download of the DMG for the current architecture, with signature, bundle ID, version and team validation.
- **Added:** Restart & Update after download; Fathom replaces its own app bundle and relaunches.
- **Improved:** Release notes render as plain text, with readable errors and retry actions in the dialog.

## What 0.0.1 does

### Draw the plan

- Draw bearing, exterior, partition and low walls, with exact length, thickness and height input.
- Grid snapping and Shift orthographic constraint for walls, edits and measurements.
- Place doors and windows independently, with exact width, height, sill and wall position; doors support hinge-side and opening-direction changes.
- Bearing walls are protected from demolition; non-bearing walls can be removed.
- Closed rooms are detected automatically, with net area and room renaming.
- Select, marquee-select, multi-select, copy, paste and delete; align several furniture items together.
- Toggle room names, furniture, grid, dimensions, bearing-wall marks and the plan underlay independently.
- Drawing scales from 1:N, fit-to-window and fullscreen.

### Furnish the plan

- 70 built-in furniture items across living, bedroom, dining, bath, appliances and study categories.
- Library with category filters, search, favourites and favourites-only view.
- Drag from the library into the plan; furniture avoids walls and other pieces instead of overlapping.
- Free rotation, 5° fine rotation and 90° quick rotation.
- Exact width, depth, X and Y editing; bring-to-front, send-to-back, duplicate and bulk delete.

### View it in 3D

- Switch between 2D, 3D and first-person walkthrough at any time.
- Orbit, wheel zoom, and top/bird/oblique presets.
- Wall cutaway, a 06:00–18:00 sunlight timeline and night lighting.
- Click a room in the list to fly to it.
- Walk with WASD, mouse look, Shift speed-up and wall/furniture collision.
- Doors and windows render as real openings; furniture, materials and lighting follow the plan.

### Areas, materials and quotes

- Per-room net area, total floor area and flooring area.
- Selectable flooring materials with per-room quantities.
- Quotes include 5% material waste, material subtotal, waste cost, door/window counts and furniture count.

### Files and output

- New blank plan or start from 1-bed, 2-bed and demo templates.
- Open, Save As, autosave, recent plans and plan archiving.
- Plan info: name, client, address and notes.
- Import a floor-plan image as an underlay and detect horizontal/vertical walls; doors, windows and curves are completed by hand.
- Export PNG, SVG, A4 PDF, quote CSV and a 3D still; print through the system print dialog.
- Chinese / English UI, full undo/redo and keyboard shortcuts.

## Install

1. On Apple Silicon download `Fathom-v0.0.2-arm64.dmg`; on Intel download `Fathom-v0.0.2-x86_64.dmg`.
2. Open the DMG and drag `Fathom.app` into Applications.
3. The build is not notarized. If Gatekeeper blocks the first launch, right-click the app and choose Open.

## Requirements

- macOS 11.0 Big Sur or later
- Apple Silicon or Intel; separate builds are provided for each
- Plans are stored in `~/Documents/Fathom/`

## Built for

- Interior designers presenting and iterating schemes
- Renovation teams turning a site measurement into a quick layout and quote
- Custom-furniture studios checking cabinetry and furniture against the room
- Homeowners planning their own renovation

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for the complete 0.0.1 feature list. From 0.0.2 onward each release is recorded under Added, Improved and Bug fixes.

## Development

Source layout, build and macOS packaging live in [app/README.md](app/README.md).

## License

GNU General Public License v3.0, SPDX identifier `GPL-3.0-only`. See [LICENSE](LICENSE) for the full terms and [NOTICE](NOTICE) for the copyright notice.

Copyright (C) 2026 gexiaosen

