# UI: why the multiplayer button never appeared

Two separate faults, both now fixed. Written down because the first one wasted several cycles and is
the kind of thing that will happen again.

## Fault 1: the pak in the install was never updated

This is the whole explanation for "the race replay button was still exactly the same as before".

| | size | mtime |
|---|---|---|
| built `pak/CoopBridge_P.pak` | 50,017 | Oct 27 |
| installed `Content/Paks/CoopBridge_P.pak` | 17,602 | Oct 26 19:55 |

The install was running a two-day-old pak. Every UI change made in those sessions was in the build and
in none of the running game, so there was nothing to see. The pak is held open by the game process,
so a copy while the game is running fails, and the failure was not being noticed.

**Fixed by** `tools/deploy_pak.py`, which will not report success unless the installed pak is
byte-identical to the built one (`--check` verifies without deploying, and also catches a stale
install on the other machine). Verified by pointing `--check --game <fake dir>` at a deliberately
wrong pak: it fails with both sizes reported.

## Fault 2: `project/apps/FrontendRouter` does not exist

The module list was:

```js
window.require([
  ..., "project/apps/FrontendRouter", ...
], onSuccess, onError)
```

There is no such module in the export. RequireJS fails the whole request when any id is missing, so
`onError` fired, `ctx` stayed null and `boot()` returned before patching anything - silently, because
the only trace was a `console.log` nobody could read in-game.

What actually exists:

* `FrontendApp.js` → `preact.render(h(FrontendModule), document.body)`
* `FrontendModule.render` → `FrontendData.screensLUT[state.selectedMenu].component`
* `FrontendNavigation.set(screenID, extraProps)` → pushes history, dispatches `onChange`, sends
  `FrontendScreenChanged`

So the seam is **`FrontendData.screensLUT`**: adding an entry registers a screen that
`FrontendModule` renders like any built-in one. No router module, no monkey-patched router, no DOM
overlay.

## Three more bugs found while fixing it

1. **`preact.h` deletes `attributes.children`.** Children live on `vnode.children`. The old tree walk
   read `props.children`, which is always `undefined` on a freshly built vnode, so it could never
   have found the buttons column even if it had run. `props.children` only exists inside a component
   instance, set by preact from the vnode when it renders.

2. **`EMPTY_CHILDREN` is shared.** `preact.h` reuses one frozen-in-practice array for every childless
   vnode, so pushing into a vnode's children would corrupt every empty vnode in the tree. The tree is
   rebuilt (`prependTo` / `cloneVnode`) instead. A test renders twice and asserts exactly one entry
   each time.

3. **`MainMenu` is a decorator.** The module ends with
   `exports.MainMenu = Focusable.decorate(_MainMenu)`, and the decorator's `render` returns
   `h(_MainMenu, {...})` - one child, no menu boxes. Patching the decorator's render and searching its
   tree finds nothing. The inner class is recovered from `vnode.nodeName` on first render and patched
   once.

Also fixed: `preact.Component` is an ES class, so `Component.call(this)` throws. The screen is a real
`class Screen extends preact.Component` now. That one was caught by `tools/menu_test.js` before it
reached the game.

## Where the entry lives now

`MainMenu_rightBox` holds the replay carousel and the build number. The Multiplayer entry is prepended
above the carousel, so the left column keeps the game's own buttons untouched. The carousel is scaled
to 60% with a transform (it is `.Carousel_root.withContentBox`, 25.5x16.1rem) rather than resized, so
the component's own layout maths is left alone.

The screen itself is built from the game's components, not hand-rolled HTML:

| piece | component |
|---|---|
| page frame, focus module | `FDiv` + `TeamSelectMenu_root` |
| backdrop | `StandardBackground` |
| mode choices | `MenuButton` (same `defaultSize crossesBR shadow` modifiers as Load/New Career) |
| team picking | `TeamIconToggle` - the same control the game's own team select uses |
| Host / Join / New career / Load save | `SimpleButton` |
| title + back button | `TeamManagementHeader` with `alwaysShowBackButton` |

`TeamSelectMenu.css` is loaded explicitly (`CSSUtil.loadCSS`) because nothing on the main-menu path
loads the `TeamSelectMenu` module that would normally pull it in.

## Iterating without a restart

`lobby.js` now polls `127.0.0.1:45999/lobby.rev` and re-fetches `/lobby.js` when it changes, the same
way `bridge.js` already re-fetched itself. `tools/coop_server.py` serves both. So a UI change is: save
the file, wait ~3s, and the screen re-registers and the menu re-wraps - no repak, no restart.

The repak is still needed for a fresh install or a new machine. Keep using `deploy_pak.py` for that.

## Verified

`node tools/menu_test.js` - 29 checks, 0 failed. It checks, among other things:

* every module id the file asks for exists in the export (this would have caught `FrontendRouter`)
* the `Component` stub is a real ES class, as the shipped preact is
* the entry lands as the first child of `MainMenu_rightBox` and the carousel survives
* the left column is untouched, and repeat renders do not stack entries
* the screen registers, renders, offers all 10 real `EF1Team` ids, and has a back button, Host, Join,
  New career and Load save

Not verified: that Cohtml renders the result on screen. That needs a game launch.

## Still unknown

* Whether `screensLUT` entries added after `FrontendModule` mounted are picked up without a re-render.
  `FrontendNavigation.set` calls `setState`, which re-renders, so it should be - unproven until played.
* Whether `TeamManagementHeader` expects a `focusModulePriority` context that the main menu provides.
* What the game does when the co-op screen is left with the keyboard `E` shortcut in
  `FrontendModule.onWindowKeyDown`.
