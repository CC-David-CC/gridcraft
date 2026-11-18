# GridCraft document host

This entrypoint embeds the real Rust workbook editor in the existing Desktop. The
native workbook and its native commands remain authoritative. The JavaScript is
host integration, authorized by the WebOS integration task; it is not a second
spreadsheet implementation or a replacement standalone app.

`@webos/gridcraft` exports synchronous `mount`, returning `element`, `ready`,
`dispose`, `focus`, `inspect`, `execute`, `requestClose`, `lifecycle`, `fileBinding`,
`retry` and `checkpoint`. It takes the same public host inputs as PdfCraft. Desktop
owns resource activation, independent document windows, New/Open/Close and saved
layout. Accounts supplies caller authority; Storage supplies confirmed conditional
writes. `@webos/gridcraft/recovery` exports the versioned draft codec and public
Runtime journal/store construction.

The adapter requires exact instance/session/document/resource targets. Public
commands cannot manufacture native save acknowledgements, recovery adoption or
pause leases. The native coordination surface is for this trusted embedding
adapter; same-origin source ownership is not a security sandbox.

## Native persistence and recovery

The primary account document is XLSX. Save prepares native bytes with the pending
cell/formula edit committed through the existing domain. A native ticket retains
the exact workbook and saved baseline until durable confirmation. Later edits and
Undo/Redo remain in place. Save As adopts the confirmed full account path only
after acknowledgement. Failures retain the prior binding. Unknown outcomes retain
the pending ticket and block blind retries. Hosted AutoSave never writes directly
through the standalone filesystem engine. Backstage Browse and keyboard Save As
use the same native domain hook.

The existing native AutoSave switch controls account scheduling in hosted mode,
defaulting On for a new account profile and preserving an explicit saved Off. It
uses the public Runtime scheduler and the same conditional native Save. It starts after ten seconds of committed-edit inactivity, keeps a sixty-second
confirmed-write interval, and bounds continuous eligible editing to a two-minute
wait. Browser-local checkpoints continue independently. Unfinished native editors,
composition, dialogs, undo groups and close leases make background Save ineligible;
the native domain rechecks immediately before capture and never finishes a live
editor for a timer. Untitled/recovered-copy destinations never open timer dialogs.
An offline browser detected before native Save dispatch retries without attempting
a write. After dispatch, ordinary network errors never imply a safe retry. Only
the two verified current Storage allowance/queue rejection messages with
HTTP 429 qualify for exponential no-write retries (30 seconds through 15 minutes,
with jitter). Status alone, timeouts, uncertain writes and conflicts pause instead.
A manual confirmed Save/Save As resolves the pause and applies the same cooldown.
Automatic uploads share Runtime's two-write page limit; manual Save remains direct.

An unexpected reload of the same iframe is fenced by its original native Document
identity. Old async operations cannot acknowledge a replacement editor. The view
becomes explicitly unavailable and can close/reopen while retaining its earlier
browser-local journal; reload does not claim a newly saved native workbook.

A clean untouched blank workbook closes quietly and remains explicitly saveable.
Recovery snapshots the native XLSX workbook and pending cell/formula text without
committing the live draft or changing its Undo history. Recovery is a local
checkpoint, separate from an account save. It deliberately refuses protected
workbooks and unfinished text-box/macro editors, retaining the prior journal
checkpoint on failure. Restored work starts untrusted and dirty; changed saved
bases require Save As. Native Undo history, incomplete IME composition and every
advanced workbook feature are not promised by this schema. Schema version 1 is
bounded to 128 MiB. Encrypted account workbooks are explicitly unsupported.

Preferences and recent paths use only the supplied issuer/account profile. An
absent or invalid hosted profile disables preference persistence; standalone
origin-global preferences are never imported into an account. Uploaded account
font management is not implemented; the browser graph uses the explicit pinned
font set and the native workbook's existing formatting choices.

## Reproducible browser graph

Use a reviewed clean source commit after native quality gates. The original
workspace must be unavailable. Supply explicit self-contained local bare providers
for every Git fork in `Cargo.lock`, the clean fonts checkout pinned in
`craft-fonts.rev`, an external prepared Cargo home/cache and a new output path.
Do not fetch dependencies during the offline build or install into the source.

```sh
python3 webos/build.py --output /external/new-gridcraft-artifact \
  --target-dir /external/gridcraft-target --fonts /inputs/craft-fonts \
  --craft-libs-origin /inputs/craft-libs.git --egui-origin /inputs/egui.git \
  --winit-origin /inputs/winit.git --winprint-origin /inputs/winprint.git --jobs 8
```

The toolchain is `nightly-2026-11-16`, target `wasm32-unknown-unknown`, development
profile without debug info, matching `wasm-bindgen 0.2.129`. The recipe uses Cargo
and wasm-bindgen directly; no Trunk or optimizer installation is required. It
verifies all provider commits in the actual lock, builds locked/offline, packages
one native WASM graph with an external module loader and CSS, copies notices and
hashes every payload in `component-build.json`. It refuses source/cache/output
collisions, a dirty source, an existing output and an insufficient disk reserve.
Account/browser acceptance is separate from production of this graph.

Only `craft-web-host` advances to the reviewed WebOS provider commit
`94981849457596b032355f74144cdd91c04d5df5` for identified saves. Other craft-libs
pins remain `aa84415b…` and `bb57d422…`; their actual APIs are not presumed
interchangeable. Localization is compiled from this repository. There are no
external translation or model inputs in this browser graph.

## Format and provenance limits

XLSX is the first host workflow. CSV/TSV, XLS/XLSB/ODS, encryption and advanced
pass-through fidelity are not qualified account-edit formats. Native CSV/HTML/PDF
exports remain browser copies and cannot rebind or mark a document saved. Each
export's delivery/fidelity and browser printing require their own browser checks;
PDF printing is not claimed under the hosted CSP.

About, contributors, model credits and community links remain. The hosted graph
contains no restricted ArtCraft brand artwork. About describes this modified
build as based on GridCraft by the ArtCraft team and community. Source licences,
asset attribution and font notices accompany the graph.

## Qualification status

Source integration is in progress. Adapter tests use the actual Runtime scheduler and exercise synthetic native/Storage transport;
they do not qualify native XLSX fidelity. Native host tests cover delayed saves,
Undo/Redo, stale replies, unknown outcomes, native pause and pending formula
recovery, including a two-sheet/formula/style round trip. An initial native typecheck passed before the account-toggle follow-up. Current
native quality gates, actual compiled browser/account workflows and parent acceptance must run before
any integration commit/promotion. No deployment is authorized by this document.

Small checks need no installation:

```sh
WEBOS_RUNTIME_SOURCE=/inputs/runtime node --import ./webos/tests/register-runtime.mjs --experimental-vm-modules --test webos/tests/host-boundary.test.mjs
WEBOS_RUNTIME_SOURCE=/inputs/runtime node --import ./webos/tests/register-runtime.mjs --test webos/tests/drafts.test.mjs
PYTHONDONTWRITEBYTECODE=1 python3 webos/tests/test_build.py
```
